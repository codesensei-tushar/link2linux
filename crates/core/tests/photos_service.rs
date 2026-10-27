//! The photos service on desktop sessions, against phones each test
//! scripts frame by frame.

mod session_common;

use futures_util::{FutureExt, SinkExt, StreamExt};
use link2linux_core::photos::{PhotoError, PhotoFile, PhotoService, PhotosState, Thumbnail};
use link2linux_core::session::SessionStart;
use link2linux_core::{DeviceInfo, DeviceKind, Feature, Session, SessionConfig, SessionEvent};
use link2linux_protocol::v1::{
    self as wire, ErrorCode, ErrorReply, FeatureAvailability, FeatureState, FeatureStatus,
    ImageFormat, PhotoListChanged, PhotoListReply,
};
use link2linux_protocol::{Envelope, Hello, PROTOCOL_VERSION, Payload};
use session_common::{RawPeer, answer, envelope, pipe, read_announcement};
use tokio::io::DuplexStream;
use tokio::sync::oneshot;
use tokio::time::{Duration, Instant};

use FeatureAvailability::{Disabled, PermissionRequired, Ready};

/// A desktop that does not ping, so only what a test scripts goes over the
/// pipe.
fn config() -> SessionConfig {
    SessionConfig {
        ping: None,
        ..SessionConfig::default()
    }
}

fn photo(id: &str, modified: i64) -> wire::Photo {
    wire::Photo {
        id: id.into(),
        taken_unix_ms: modified,
        modified_unix_ms: modified,
        name: format!("{id}.jpg"),
        mime_type: "image/jpeg".into(),
        size: 10,
        width: 4000,
        height: 3000,
    }
}

fn list(photos: Vec<wire::Photo>) -> Payload {
    Payload::PhotoListReply(PhotoListReply { photos })
}

fn thumbnail(data: &[u8]) -> Payload {
    Payload::PhotoThumbnail(wire::PhotoThumbnail {
        format: ImageFormat::Jpeg.into(),
        data: data.to_vec(),
        width: 256,
        height: 192,
    })
}

fn chunk(size: u64, modified: i64, data: &[u8]) -> Payload {
    Payload::PhotoData(wire::PhotoData {
        size,
        modified_unix_ms: modified,
        data: data.to_vec(),
    })
}

fn refusal(code: ErrorCode, retry_after_ms: u32) -> Payload {
    Payload::ErrorReply(ErrorReply {
        code: code.into(),
        retry_after_ms,
        ..ErrorReply::default()
    })
}

/// One phone's session, the phone's end of it, and the desktop's photos
/// service for that phone.
struct Link {
    session: Session<DuplexStream>,
    phone: RawPeer,
    photos: PhotoService,
    /// The phone's next sequence number.
    seq: u64,
}

impl Link {
    /// A session with a phone that lists `features` and has not reported
    /// them yet, driving `photos`.
    async fn connect_with(features: &[&str], mut photos: PhotoService) -> Self {
        let desktop = DeviceInfo::new("desktop-1", "Desk", DeviceKind::Desktop, [Feature::Photos]);
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: "phone-1".into(),
            device_name: "Pixel".into(),
            kind: DeviceKind::Phone.into(),
            features: features.iter().map(|&feature| feature.into()).collect(),
            ..Hello::default()
        };
        let (io, mut phone) = pipe();
        let script = async {
            phone.next().await.unwrap().unwrap();
            phone
                .send(&envelope(1, Payload::Hello(hello)))
                .await
                .unwrap();
            read_announcement(&mut phone).await;
        };
        let (session, ()) = tokio::join!(
            Session::handshake(io, SessionStart::new(desktop), config()),
            script
        );
        let mut session = session.unwrap();
        photos.session_started(&mut session).await.unwrap();
        Self {
            session,
            phone,
            photos,
            seq: 2,
        }
    }

    async fn connect(features: &[&str]) -> Self {
        Self::connect_with(features, PhotoService::new()).await
    }

    /// A session where the phone reported photos ready and answered the
    /// list request with `photos`.
    async fn listed(photos: Vec<wire::Photo>) -> Self {
        Self::listed_with(photos, PhotoService::new()).await
    }

    async fn listed_with(photos: Vec<wire::Photo>, service: PhotoService) -> Self {
        let mut link = Self::connect_with(&["photos"], service).await;
        link.report(&[("photos", Ready, "")]).await;
        let asked = link.request().await;
        assert!(matches!(asked.1, Payload::PhotoListRequest(_)));
        link.answer(asked.0, list(photos)).await;
        assert_eq!(link.photos.state(), PhotosState::Listed);
        link
    }

    /// The phone sends `frame`, and the desktop takes it. Both at once:
    /// the pipe holds 64 KiB, less than a thumbnail or a chunk.
    async fn send(&mut self, frame: Envelope) -> Option<SessionEvent> {
        let Self { session, phone, .. } = self;
        let (sent, event) = tokio::join!(phone.send(&frame), session.next_event());
        sent.unwrap();
        self.seq += 1;
        let event = event.unwrap().unwrap();
        self.photos
            .handle_event(&mut self.session, event)
            .await
            .unwrap()
    }

    /// The phone sends a one-way `payload`.
    async fn phone_sends(&mut self, payload: Payload) -> Option<SessionEvent> {
        self.send(envelope(self.seq, payload)).await
    }

    /// The phone answers the desktop's request `request` with `payload`.
    async fn answer(&mut self, request: u64, payload: Payload) -> Option<SessionEvent> {
        self.send(answer(self.seq, request, payload)).await
    }

    /// The phone reports its features, each with a permission token.
    async fn report(&mut self, states: &[(&str, FeatureAvailability, &str)]) {
        let status = FeatureStatus {
            features: states
                .iter()
                .map(|&(feature, availability, permission)| FeatureState {
                    feature: feature.into(),
                    availability: availability.into(),
                    permission: permission.into(),
                })
                .collect(),
        };
        let rest = self.phone_sends(Payload::FeatureStatus(status)).await;
        assert!(
            matches!(rest, Some(SessionEvent::FeatureStatus(_))),
            "{rest:?}"
        );
    }

    /// The desktop's next request: its sequence number and payload.
    async fn request(&mut self) -> (u64, Payload) {
        let frame = self.phone.next().await.unwrap().unwrap();
        assert!(frame.expects_reply, "{frame:?}");
        (frame.seq, frame.payload.unwrap())
    }

    /// Reads `count` thumbnail requests: their sequence numbers and ids.
    async fn thumbnail_requests(&mut self, count: usize) -> Vec<(u64, String)> {
        let mut requests = Vec::new();
        for _ in 0..count {
            match self.request().await {
                (seq, Payload::PhotoThumbnailRequest(request)) => requests.push((seq, request.id)),
                other => panic!("expected a thumbnail request, got {other:?}"),
            }
        }
        requests
    }

    /// Reads `count` data requests: their sequence numbers and offsets.
    async fn data_requests(&mut self, count: usize) -> Vec<(u64, u64)> {
        let mut requests = Vec::new();
        for _ in 0..count {
            match self.request().await {
                (seq, Payload::PhotoDataRequest(request)) => requests.push((seq, request.offset)),
                other => panic!("expected a data request, got {other:?}"),
            }
        }
        requests
    }

    /// Fails if the desktop sent anything the phone has not read.
    fn sent_nothing(&mut self) {
        if let Some(frame) = self.phone.next().now_or_never() {
            panic!("the desktop sent {frame:?}");
        }
    }

    fn want_thumbnail(
        &mut self,
        id: &str,
        max_size: u32,
    ) -> oneshot::Receiver<Result<Thumbnail, PhotoError>> {
        let (waiter, receiver) = oneshot::channel();
        self.photos
            .thumbnail(&mut self.session, id, max_size, waiter)
            .unwrap();
        receiver
    }

    fn want_photo(&mut self, id: &str) -> oneshot::Receiver<Result<PhotoFile, PhotoError>> {
        let (waiter, receiver) = oneshot::channel();
        self.photos.fetch(&mut self.session, id, waiter).unwrap();
        receiver
    }
}

/// What `receiver` got, which must have come already.
fn got<T: std::fmt::Debug>(
    mut receiver: oneshot::Receiver<Result<T, PhotoError>>,
) -> Result<T, PhotoError> {
    receiver.try_recv().expect("no answer yet")
}

#[tokio::test]
async fn the_list_is_asked_for_once_ready_and_kept() {
    let mut odd = photo("odd", 4);
    odd.name = "../\u{202E}gpj.exe".into();
    let link = Link::listed(vec![
        photo("b", 3),
        odd,
        photo("", 2),
        photo(&"x".repeat(65), 2),
        photo("b", 1),
    ])
    .await;

    let photos = link.photos.photos();
    let ids: Vec<_> = photos.iter().map(|photo| photo.id.as_str()).collect();
    assert_eq!(ids, ["b", "odd"]);
    assert_eq!(photos[0].modified_unix_ms, 3);
    assert_eq!(photos[1].name, "__gpj.exe");
}

#[tokio::test]
async fn why_there_is_no_list() {
    let mut link = Link::connect(&["photos"]).await;
    assert_eq!(link.photos.state(), PhotosState::Waiting);

    link.report(&[("photos", Disabled, "")]).await;
    assert_eq!(link.photos.state(), PhotosState::TurnedOff);

    link.report(&[("photos", PermissionRequired, "photos")])
        .await;
    assert_eq!(link.photos.state(), PhotosState::PermissionRequired);
    link.sent_nothing();

    let mut older = Link::connect(&["notifications"]).await;
    older.report(&[("notifications", Ready, "")]).await;
    assert_eq!(older.photos.state(), PhotosState::Unsupported);
}

#[tokio::test]
async fn a_refused_list_says_why() {
    let mut link = Link::connect(&["photos"]).await;
    link.report(&[("photos", Ready, "")]).await;
    let (seq, _) = link.request().await;

    link.answer(seq, refusal(ErrorCode::PermissionRequired, 0))
        .await;

    assert_eq!(link.photos.state(), PhotosState::PermissionRequired);
}

#[tokio::test]
async fn changes_replace_the_list() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;

    link.phone_sends(Payload::PhotoListChanged(PhotoListChanged {
        photos: vec![photo("b", 2), photo("a", 1)],
    }))
    .await;

    let ids: Vec<_> = link.photos.photos().iter().map(|p| p.id.clone()).collect();
    assert_eq!(ids, ["b", "a"]);
}

#[tokio::test]
async fn a_change_before_asking_is_ignored() {
    let mut link = Link::connect(&["photos"]).await;

    link.phone_sends(Payload::PhotoListChanged(PhotoListChanged {
        photos: vec![photo("a", 1)],
    }))
    .await;

    assert!(link.photos.photos().is_empty());
}

#[tokio::test]
async fn a_thumbnail_is_fetched_once_and_then_kept() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;

    let first = link.want_thumbnail("a", 4096);
    let (seq, request) = link.request().await;
    let Payload::PhotoThumbnailRequest(request) = request else {
        panic!("{request:?}");
    };
    assert_eq!((request.id.as_str(), request.max_size), ("a", 2048));
    link.answer(seq, thumbnail(b"jpeg")).await;

    let thumbnail = got(first).unwrap();
    assert_eq!(thumbnail.mime_type, "image/jpeg");
    assert_eq!(&*thumbnail.data, b"jpeg");
    // From memory: nothing more goes to the phone.
    assert_eq!(&*got(link.want_thumbnail("a", 2048)).unwrap().data, b"jpeg");
    link.sent_nothing();
}

#[tokio::test]
async fn only_listed_photos_are_asked_for() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;

    assert_eq!(
        got(link.want_thumbnail("elsewhere", 256)),
        Err(PhotoError::NotFound)
    );
    assert_eq!(
        got(link.want_photo("elsewhere")).unwrap_err(),
        PhotoError::NotFound
    );
    link.sent_nothing();
}

#[tokio::test]
async fn at_most_eight_thumbnails_wait_and_the_newest_wish_goes_first() {
    let photos = (0..10).map(|i| photo(&format!("p{i}"), 1)).collect();
    let mut link = Link::listed(photos).await;

    let receivers: Vec<_> = (0..10)
        .map(|i| link.want_thumbnail(&format!("p{i}"), 256))
        .collect();
    let first = link.thumbnail_requests(8).await;
    link.sent_nothing();
    // Each wish asks at once while there is room, so the first eight go in order.
    assert_eq!(first[0].1, "p0");

    link.answer(first[0].0, thumbnail(b"0")).await;
    let next = link.thumbnail_requests(1).await;
    assert_eq!(next[0].1, "p9");
    assert_eq!(
        &*got(receivers.into_iter().next().unwrap()).unwrap().data,
        b"0"
    );
}

#[tokio::test]
async fn a_thumbnail_nobody_waits_for_is_not_asked_for() {
    let photos = (0..9).map(|i| photo(&format!("p{i}"), 1)).collect();
    let mut link = Link::listed(photos).await;
    let kept: Vec<_> = (0..8)
        .map(|i| link.want_thumbnail(&format!("p{i}"), 256))
        .collect();
    let first = link.thumbnail_requests(8).await;

    drop(link.want_thumbnail("p8", 256));
    link.answer(first[0].0, thumbnail(b"0")).await;

    link.sent_nothing();
    drop(kept);
}

#[tokio::test(start_paused = true)]
async fn a_busy_phone_is_asked_again_later() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_thumbnail("a", 256);
    let (seq, _) = link.request().await;

    link.answer(seq, refusal(ErrorCode::Busy, 2_000)).await;
    let deadline = link.photos.next_deadline().unwrap();
    assert!(deadline >= Instant::now() + Duration::from_millis(1_900));
    link.photos
        .tick(&mut link.session, Instant::now())
        .await
        .unwrap();
    link.sent_nothing();

    tokio::time::advance(Duration::from_secs(2)).await;
    link.photos
        .tick(&mut link.session, Instant::now())
        .await
        .unwrap();
    let again = link.thumbnail_requests(1).await;
    link.answer(again[0].0, thumbnail(b"a")).await;
    assert!(got(receiver).is_ok());
}

#[tokio::test]
async fn a_new_version_is_fetched_anew() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_thumbnail("a", 256);
    let (seq, _) = link.request().await;
    link.answer(seq, thumbnail(b"old")).await;
    got(receiver).unwrap();

    link.phone_sends(Payload::PhotoListChanged(PhotoListChanged {
        photos: vec![photo("a", 2)],
    }))
    .await;
    let receiver = link.want_thumbnail("a", 256);
    let (seq, _) = link.request().await;
    link.answer(seq, thumbnail(b"new")).await;

    assert_eq!(&*got(receiver).unwrap().data, b"new");
}

#[tokio::test]
async fn a_thumbnail_that_breaks_the_rules_is_not_shown() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_thumbnail("a", 256);
    let (seq, _) = link.request().await;

    link.answer(seq, thumbnail(&vec![0; 400 * 1024 + 1])).await;

    assert_eq!(got(receiver), Err(PhotoError::Failed));
}

#[tokio::test]
async fn a_photo_comes_in_chunks_four_at_a_time() {
    let file: Vec<u8> = (0..1_300 * 1024).map(|i| (i % 251) as u8).collect();
    let size = file.len() as u64;
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");

    let first = link.data_requests(1).await;
    assert_eq!(first[0].1, 0);
    link.sent_nothing();
    link.answer(first[0].0, chunk(size, 5, &file[..256 * 1024]))
        .await;
    let more = link.data_requests(4).await;
    link.sent_nothing();
    let offsets: Vec<_> = more.iter().map(|&(_, offset)| offset).collect();
    assert_eq!(offsets, [256 * 1024, 512 * 1024, 768 * 1024, 1024 * 1024]);
    for (seq, offset) in more {
        let start = usize::try_from(offset).unwrap();
        let end = (start + 256 * 1024).min(file.len());
        link.answer(seq, chunk(size, 5, &file[start..end])).await;
    }
    let last = link.data_requests(1).await;
    assert_eq!(last[0].1, 1_280 * 1024);
    link.answer(last[0].0, chunk(size, 5, &file[1_280 * 1024..]))
        .await;

    let photo = got(receiver).unwrap();
    assert_eq!(photo.name, "a.jpg");
    assert_eq!(photo.modified_unix_ms, 5);
    assert_eq!(&*photo.data, &file[..]);
}

#[tokio::test]
async fn an_empty_photo_comes_in_one_answer() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");
    let first = link.data_requests(1).await;

    link.answer(first[0].0, chunk(0, 5, b"")).await;

    assert!(got(receiver).unwrap().data.is_empty());
}

#[tokio::test]
async fn a_photo_that_changes_starts_again_once() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");
    let first = link.data_requests(1).await;
    link.answer(first[0].0, chunk(512 * 1024, 5, &[1; 256 * 1024]))
        .await;
    let second = link.data_requests(1).await;

    // Changed on the phone: from the start again.
    link.answer(second[0].0, chunk(300 * 1024, 6, &[2; 44 * 1024]))
        .await;
    let again = link.data_requests(1).await;
    assert_eq!(again[0].1, 0);
    link.answer(again[0].0, chunk(300 * 1024, 6, &[2; 256 * 1024]))
        .await;
    let rest = link.data_requests(1).await;

    // And once more: that is too often.
    link.answer(rest[0].0, chunk(300 * 1024, 7, &[2; 44 * 1024]))
        .await;
    assert_eq!(got(receiver).unwrap_err(), PhotoError::Changed);
}

#[tokio::test(start_paused = true)]
async fn a_change_after_the_list_request_timed_out_is_the_list() {
    let mut link = Link::connect(&["photos"]).await;
    link.report(&[("photos", Ready, "")]).await;
    let (seq, _) = link.request().await;

    let event = link.session.next_event().await.unwrap().unwrap();
    assert!(matches!(event, SessionEvent::RequestTimedOut { request } if request == seq));
    link.photos
        .handle_event(&mut link.session, event)
        .await
        .unwrap();
    assert_eq!(link.photos.state(), PhotosState::Unavailable);
    assert!(link.photos.next_deadline().is_some());

    link.phone_sends(Payload::PhotoListChanged(PhotoListChanged {
        photos: vec![photo("a", 1)],
    }))
    .await;

    assert_eq!(link.photos.state(), PhotosState::Listed);
    assert_eq!(link.photos.photos().len(), 1);
    assert!(link.photos.next_deadline().is_none());
}

#[tokio::test]
async fn a_new_version_drops_what_a_fetch_already_holds() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");
    let first = link.data_requests(1).await;
    link.answer(first[0].0, chunk(512 * 1024, 1, &[1; 256 * 1024]))
        .await;
    let stale = link.data_requests(1).await;
    assert_eq!(stale[0].1, 256 * 1024);

    link.phone_sends(Payload::PhotoListChanged(PhotoListChanged {
        photos: vec![photo("a", 2)],
    }))
    .await;
    let again = link.data_requests(1).await;
    assert_eq!(again[0].1, 0);
    // The chunk asked for before the photo changed does not finish the fetch.
    link.answer(stale[0].0, chunk(512 * 1024, 1, &[1; 256 * 1024]))
        .await;
    link.answer(again[0].0, chunk(4, 2, b"new!")).await;

    let photo = got(receiver).unwrap();
    assert_eq!(photo.modified_unix_ms, 2);
    assert_eq!(&*photo.data, b"new!");
}

#[tokio::test]
async fn a_photo_that_leaves_the_list_is_not_fetched() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");
    let first = link.data_requests(1).await;

    link.phone_sends(Payload::PhotoListChanged(PhotoListChanged {
        photos: vec![],
    }))
    .await;

    assert_eq!(got(receiver).unwrap_err(), PhotoError::NotFound);
    link.answer(first[0].0, chunk(10, 1, &[1; 10])).await;
    link.sent_nothing();
}

#[tokio::test]
async fn a_short_chunk_fails_the_photo() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");
    let first = link.data_requests(1).await;

    link.answer(first[0].0, chunk(512 * 1024, 5, &[1; 1000]))
        .await;

    assert_eq!(got(receiver).unwrap_err(), PhotoError::Failed);
}

#[tokio::test]
async fn a_deleted_photo_is_not_found() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_photo("a");
    let first = link.data_requests(1).await;

    link.answer(first[0].0, refusal(ErrorCode::NotFound, 0))
        .await;

    assert_eq!(got(receiver).unwrap_err(), PhotoError::NotFound);
}

#[tokio::test]
async fn photos_turned_off_drop_everything() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_thumbnail("a", 256);
    let (seq, _) = link.request().await;
    link.answer(seq, thumbnail(b"a")).await;
    got(receiver).unwrap();
    let waiting = link.want_photo("a");
    link.data_requests(1).await;

    link.report(&[("photos", Disabled, "")]).await;

    assert_eq!(got(waiting).unwrap_err(), PhotoError::NotReady);
    assert!(link.photos.photos().is_empty());
    assert_eq!(
        got(link.want_thumbnail("a", 256)),
        Err(PhotoError::NotReady)
    );
    // Ready again: the list is asked for again, and the thumbnail too.
    link.report(&[("photos", Ready, "")]).await;
    let (seq, _) = link.request().await;
    link.answer(seq, list(vec![photo("a", 1)])).await;
    let _receiver = link.want_thumbnail("a", 256);
    link.thumbnail_requests(1).await;
}

#[tokio::test]
async fn thumbnails_stay_for_the_next_session() {
    let mut link = Link::listed(vec![photo("a", 1)]).await;
    let receiver = link.want_thumbnail("a", 256);
    let (seq, _) = link.request().await;
    link.answer(seq, thumbnail(b"a")).await;
    got(receiver).unwrap();
    let waiting = link.want_photo("a");
    link.data_requests(1).await;

    let mut service = link.photos;
    service.session_ended();
    assert_eq!(got(waiting).unwrap_err(), PhotoError::NotConnected);
    assert_eq!(service.state(), PhotosState::Disconnected);

    let mut again = Link::listed_with(vec![photo("a", 1)], service).await;
    assert_eq!(&*got(again.want_thumbnail("a", 256)).unwrap().data, b"a");
    again.sent_nothing();
}
