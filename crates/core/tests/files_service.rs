//! The files service on a desktop session: against a phone each test
//! scripts frame by frame, and against a second service standing in for
//! the phone.

mod session_common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aws_lc_rs::digest;
use futures_util::{SinkExt, StreamExt};
use link2linux_core::files::{
    CHUNK_LEN, Direction, Failure, FileService, KEEP_UNFINISHED, SendError, Transfer,
    TransferEvent, TransferId, TransferState,
};
use link2linux_core::session::{Availability, SessionStart};
use link2linux_core::{Feature, Session, SessionConfig};
use link2linux_protocol::v1::{
    Ack, ErrorCode, ErrorReply, FileAccept, FileCancel, FileChunk, FileInfo, FileOffer, Pong,
};
use link2linux_protocol::{Envelope, Payload};
use session_common::{
    RawPeer, all_ready, desktop, envelope, phone, phone_hello, pipe, read_announcement,
};
use tokio::io::{DuplexStream, duplex};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

const KIB: usize = 1024;

/// The files of an offer: names and contents.
type Files<'a> = &'a [(&'a str, &'a [u8])];

/// The phone's end of the pipe, scripted by each test.
struct Phone {
    peer: RawPeer,
    seq: u64,
}

impl Phone {
    async fn send(&mut self, payload: Payload, expects_reply: bool, reply_to: u64) -> u64 {
        self.seq += 1;
        let frame = Envelope {
            seq: self.seq,
            expects_reply,
            reply_to,
            payload: Some(payload),
        };
        self.peer.send(&frame).await.unwrap();
        self.seq
    }

    async fn request(&mut self, payload: Payload) -> u64 {
        self.send(payload, true, 0).await
    }

    async fn answer(&mut self, request: u64, payload: Payload) {
        self.send(payload, false, request).await;
    }

    async fn ack(&mut self, request: u64) {
        self.answer(request, Payload::Ack(Ack {})).await;
    }

    /// The desktop's next frame, answering pings on the way.
    async fn next(&mut self) -> Envelope {
        loop {
            let frame = timeout(Duration::from_secs(10), self.peer.next())
                .await
                .expect("a frame in time")
                .expect("the desktop is connected")
                .unwrap();
            if let Some(Payload::Ping(ping)) = &frame.payload {
                let nonce = ping.nonce;
                self.send(Payload::Pong(Pong { nonce }), false, 0).await;
                continue;
            }
            return frame;
        }
    }

    /// Checks that the desktop sends nothing for a while.
    async fn quiet(&mut self) {
        let frame = timeout(Duration::from_millis(300), self.peer.next()).await;
        assert!(frame.is_err(), "unexpected frame {frame:?}");
    }

    /// Sends request `payload` and returns the desktop's answer.
    async fn ask(&mut self, payload: Payload) -> Payload {
        let request = self.request(payload).await;
        let answer = self.next().await;
        assert_eq!(answer.reply_to, request, "{answer:?}");
        answer.payload.unwrap()
    }

    /// Offers `files` as transfer `id` and returns the answer.
    async fn offer(&mut self, id: [u8; 16], files: &[(&str, &[u8])]) -> Payload {
        self.ask(offer(id, files)).await
    }

    /// Sends file `index` of transfer `id` from `from` on, one chunk at a
    /// time, and returns the answers.
    async fn send_file(
        &mut self,
        id: [u8; 16],
        index: u32,
        data: &[u8],
        from: usize,
    ) -> Vec<Payload> {
        let mut answers = Vec::new();
        let mut offset = from;
        loop {
            let end = (offset + CHUNK_LEN).min(data.len());
            let last = end == data.len();
            let sha256 = if last { sha256(data) } else { Vec::new() };
            answers.push(
                self.ask(chunk(id, index, offset, &data[offset..end], sha256))
                    .await,
            );
            if last {
                return answers;
            }
            offset = end;
        }
    }
}

fn offer(id: [u8; 16], files: &[(&str, &[u8])]) -> Payload {
    Payload::FileOffer(FileOffer {
        transfer_id: id.to_vec(),
        created_unix_ms: 1_700_000_000_000,
        files: files
            .iter()
            .map(|(name, data)| FileInfo {
                name: (*name).into(),
                size: data.len() as u64,
                ..FileInfo::default()
            })
            .collect(),
    })
}

fn chunk(id: [u8; 16], index: u32, offset: usize, data: &[u8], sha256: Vec<u8>) -> Payload {
    Payload::FileChunk(FileChunk {
        transfer_id: id.to_vec(),
        file_index: index,
        offset: offset as u64,
        data: data.to_vec(),
        sha256,
    })
}

fn accept(file_index: u32, offset: u64) -> Payload {
    Payload::FileAccept(FileAccept { file_index, offset })
}

fn refusal(code: ErrorCode) -> Payload {
    Payload::ErrorReply(ErrorReply {
        code: code.into(),
        ..ErrorReply::default()
    })
}

fn code_of(payload: &Payload) -> ErrorCode {
    match payload {
        Payload::ErrorReply(error) => error.code(),
        other => panic!("expected an ErrorReply, got {other:?}"),
    }
}

fn is_ack(payload: &Payload) -> bool {
    matches!(payload, Payload::Ack(_))
}

fn sha256(data: &[u8]) -> Vec<u8> {
    digest::digest(&digest::SHA256, data).as_ref().to_vec()
}

/// Bytes that differ from one position to the next, so misplaced chunks
/// show.
fn data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) ^ (i >> 8) as u8)
        .collect()
}

/// What the user asks of the desktop in a test.
enum Command {
    Send(Vec<PathBuf>, oneshot::Sender<Result<TransferId, SendError>>),
    Cancel(TransferId),
    Transfers(oneshot::Sender<Vec<Transfer>>),
    /// The user turned files on or off here.
    Switch(Availability),
}

/// A service on its session, driven the way the runtime drives it.
struct Driven {
    commands: mpsc::UnboundedSender<Command>,
    events: broadcast::Receiver<TransferEvent>,
    task: JoinHandle<FileService>,
}

impl Driven {
    fn start(mut session: Session<DuplexStream>, mut service: FileService) -> Self {
        let events = service.subscribe();
        let (commands, mut inbox) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            service.session_started(&mut session).await.unwrap();
            loop {
                tokio::select! {
                    command = inbox.recv() => {
                        match command {
                            None => break,
                            Some(Command::Send(paths, reply)) => {
                                let _ = reply.send(service.send(paths).await);
                            }
                            Some(Command::Cancel(id)) => {
                                service.cancel(id).await;
                            }
                            Some(Command::Transfers(reply)) => {
                                let _ = reply.send(service.transfers());
                            }
                            Some(Command::Switch(availability)) => {
                                if session.set_availability(Feature::Files, availability).await.is_err() {
                                    break;
                                }
                            }
                        }
                        if service.run(&mut session).await.is_err() {
                            break;
                        }
                    }
                    event = session.next_event() => match event {
                        Ok(Some(event)) => {
                            if service.handle_event(&mut session, event).await.is_err() {
                                break;
                            }
                        }
                        Ok(None) | Err(_) => break,
                    },
                }
            }
            service.session_ended();
            service
        });
        Self {
            commands,
            events,
            task,
        }
    }

    async fn send(&self, paths: Vec<PathBuf>) -> Result<TransferId, SendError> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Send(paths, reply)).unwrap();
        answer.await.unwrap()
    }

    fn cancel(&self, id: TransferId) {
        self.commands.send(Command::Cancel(id)).unwrap();
    }

    fn switch(&self, availability: Availability) {
        self.commands.send(Command::Switch(availability)).unwrap();
    }

    async fn transfers(&self) -> Vec<Transfer> {
        let (reply, answer) = oneshot::channel();
        self.commands.send(Command::Transfers(reply)).unwrap();
        answer.await.unwrap()
    }

    async fn transfer(&self, id: TransferId) -> Transfer {
        self.transfers()
            .await
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
    }

    /// Waits for the event that changes transfer `id` to `state`.
    async fn reaches(&mut self, id: TransferId, state: TransferState) -> Transfer {
        timeout(Duration::from_secs(10), async {
            loop {
                match self.events.recv().await {
                    Ok(TransferEvent::Changed(transfer))
                        if transfer.id == id && transfer.state == state =>
                    {
                        return transfer;
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            panic!("the service stopped");
        })
        .await
        .unwrap_or_else(|_| panic!("transfer {id} never became {state:?}"))
    }

    /// Waits until transfer `id` is in `state`, whatever the events said
    /// before.
    async fn settles(&self, id: TransferId, state: TransferState) {
        timeout(Duration::from_secs(10), async {
            while self.transfer(id).await.state != state {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("transfer {id} never became {state:?}"));
    }

    /// The service, once its session is gone.
    async fn stopped(self) -> FileService {
        drop(self.commands);
        self.task.await.unwrap()
    }
}

/// A desktop session whose own files feature is `files`, and the scripted
/// phone, which reports every feature ready.
async fn connect(files: Availability, config: SessionConfig) -> (Session<DuplexStream>, Phone) {
    let (io, mut peer) = pipe();
    let mut start = SessionStart::new(desktop());
    start.availability.insert(Feature::Files, files);
    let script = async move {
        peer.next().await.unwrap().unwrap();
        peer.send(&envelope(1, Payload::Hello(phone_hello())))
            .await
            .unwrap();
        read_announcement(&mut peer).await;
        peer
    };
    let (session, peer) = tokio::join!(Session::handshake(io, start, config), script);
    let mut phone = Phone { peer, seq: 1 };
    phone
        .send(all_ready(&["clipboard", "files", "messages"]), false, 0)
        .await;
    (session.unwrap(), phone)
}

/// A desktop with the files feature on, saving into `dir`, and its phone.
async fn desktop_with(service: FileService) -> (Driven, Phone) {
    let (session, phone) = connect(Availability::Ready, SessionConfig::default()).await;
    (Driven::start(session, service), phone)
}

fn service(dir: &Path) -> FileService {
    FileService::new(dir.to_owned())
}

/// The names in `dir`.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Writes `data` to a new file `name` in `dir`.
fn source(dir: &Path, name: &str, data: &[u8]) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, data).unwrap();
    path
}

#[tokio::test]
async fn files_from_the_phone_get_safe_names_and_replace_nothing() {
    let downloads = tempfile::tempdir().unwrap();
    fs::write(downloads.path().join("photo.jpg"), b"theirs").unwrap();
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;

    let photo = data(600 * KIB, 1);
    let tricky = b"echo hi".to_vec();
    let id = [1; 16];
    let files: [(&str, &[u8]); 3] = [
        ("photo.jpg", &photo),
        ("../.hidden\u{202E}gpj.exe", &tricky),
        ("empty", &[]),
    ];
    assert_eq!(phone.offer(id, &files).await, accept(0, 0));
    for (index, (_, data)) in files.iter().enumerate() {
        let answers = phone.send_file(id, index as u32, data, 0).await;
        assert!(answers.iter().all(is_ack), "{answers:?}");
    }

    let done = desktop
        .reaches(TransferId(id), TransferState::Completed)
        .await;
    assert_eq!(done.direction, Direction::Incoming);
    assert_eq!(done.done, done.size);
    let saved: Vec<PathBuf> = done.files.iter().map(|f| f.path.clone().unwrap()).collect();
    assert_eq!(saved[0], downloads.path().join("photo (1).jpg"));
    assert_eq!(saved[1], downloads.path().join("_.hidden_gpj.exe"));
    assert_eq!(saved[2], downloads.path().join("empty"));
    assert_eq!(done.files[0].name, "photo (1).jpg");
    // Typed by their names, whatever the phone said.
    assert_eq!(done.files[0].mime_type, "image/jpeg");
    assert_eq!(done.files[2].mime_type, "");
    assert_eq!(fs::read(&saved[0]).unwrap(), photo);
    assert_eq!(fs::read(&saved[1]).unwrap(), tricky);
    assert_eq!(fs::read(&saved[2]).unwrap(), b"");
    assert_eq!(
        fs::read(downloads.path().join("photo.jpg")).unwrap(),
        b"theirs"
    );
    for path in &saved {
        let mode = fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0, "{} is executable", path.display());
    }
    assert_eq!(
        names_in(downloads.path()),
        ["_.hidden_gpj.exe", "empty", "photo (1).jpg", "photo.jpg"]
    );
}

#[tokio::test]
async fn chunks_that_do_not_fit_or_do_not_match_their_hash_end_the_transfer() {
    let downloads = tempfile::tempdir().unwrap();
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let file = data(300 * KIB, 2);
    let files: [(&str, &[u8]); 1] = [("file.bin", &file)];

    // Not where the file stands.
    assert_eq!(phone.offer([1; 16], &files).await, accept(0, 0));
    let misplaced = chunk([1; 16], 0, 1000, &file[1000..2000], Vec::new());
    assert_eq!(
        code_of(&phone.ask(misplaced).await),
        ErrorCode::InvalidArgument
    );
    desktop
        .reaches(TransferId([1; 16]), TransferState::Failed(Failure::Invalid))
        .await;
    let next = chunk([1; 16], 0, 0, &file[..1000], Vec::new());
    assert_eq!(code_of(&phone.ask(next).await), ErrorCode::NotFound);

    // A damaged file is deleted.
    assert_eq!(phone.offer([2; 16], &files).await, accept(0, 0));
    let first = chunk([2; 16], 0, 0, &file[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(first).await));
    let last = chunk([2; 16], 0, CHUNK_LEN, &file[CHUNK_LEN..], vec![0; 32]);
    assert_eq!(code_of(&phone.ask(last).await), ErrorCode::InvalidArgument);
    desktop
        .reaches(TransferId([2; 16]), TransferState::Failed(Failure::Damaged))
        .await;

    // Past the end, an empty chunk of a file with bytes, a missing or
    // misplaced hash, and a chunk too long.
    let small = [7_u8; 10];
    let cases: [(u8, Files<'_>, Payload); 5] = [
        (
            3,
            &[("a", &small)],
            chunk([3; 16], 0, 0, &[7; 20], sha256(&[7; 20])),
        ),
        (4, &[("a", &small)], chunk([4; 16], 0, 0, &[], sha256(&[]))),
        (
            5,
            &[("a", &small)],
            chunk([5; 16], 0, 0, &small, Vec::new()),
        ),
        (
            6,
            &[("a", &file)],
            chunk([6; 16], 0, 0, &file[..10], sha256(&file[..10])),
        ),
        (
            7,
            &[("a", &file)],
            chunk([7; 16], 0, 0, &file[..CHUNK_LEN + 1], Vec::new()),
        ),
    ];
    for (n, files, chunk) in cases {
        assert_eq!(phone.offer([n; 16], files).await, accept(0, 0));
        assert_eq!(
            code_of(&phone.ask(chunk).await),
            ErrorCode::InvalidArgument,
            "case {n}"
        );
    }

    let unknown = chunk([9; 16], 0, 0, &small, sha256(&small));
    assert_eq!(code_of(&phone.ask(unknown).await), ErrorCode::NotFound);
    assert_eq!(names_in(downloads.path()), Vec::<String>::new());
}

#[tokio::test]
async fn offers_that_break_the_limits_or_do_not_fit_are_refused() {
    let downloads = tempfile::tempdir().unwrap();
    let free = 100 * 1024 * 1024;
    let service = service(downloads.path()).with_free_space(move |_| Ok(free));
    let (_desktop, mut phone) = desktop_with(service).await;

    let mut short_id = offer([0; 16], &[("a", b"x")]);
    if let Payload::FileOffer(offer) = &mut short_id {
        offer.transfer_id.pop();
    }
    let long_name = "n".repeat(256);
    for bad in [
        short_id,
        offer([1; 16], &[]),
        offer([1; 16], &[("", b"x")]),
        offer([1; 16], &[(&long_name, b"x")]),
    ] {
        assert_eq!(code_of(&phone.ask(bad).await), ErrorCode::InvalidArgument);
    }

    // Four running transfers from the phone, then it is busy.
    let mib = vec![0; 1024 * 1024];
    for n in 1..=4 {
        assert_eq!(phone.offer([n; 16], &[("a", &mib)]).await, accept(0, 0));
    }
    assert_eq!(
        code_of(&phone.offer([5; 16], &[("a", &mib)]).await),
        ErrorCode::Busy
    );

    // A repeated offer must match the transfer it repeats.
    let other = phone.offer([1; 16], &[("a", &mib[1..])]).await;
    assert_eq!(code_of(&other), ErrorCode::InvalidArgument);

    for n in 1..=4 {
        let cancel = Payload::FileCancel(FileCancel {
            transfer_id: vec![n; 16],
        });
        assert!(is_ack(&phone.ask(cancel).await));
    }
    // 40 MiB fits in 100 MiB with room to spare; 50 MiB does not.
    let offer_of = |id: u8, size: u64| {
        Payload::FileOffer(FileOffer {
            transfer_id: vec![id; 16],
            files: vec![FileInfo {
                name: "big".into(),
                size,
                ..FileInfo::default()
            }],
            ..FileOffer::default()
        })
    };
    assert_eq!(
        code_of(&phone.ask(offer_of(6, 50 << 20)).await),
        ErrorCode::NoSpace
    );
    assert_eq!(phone.ask(offer_of(7, 30 << 20)).await, accept(0, 0));
    // The 30 MiB still to come count against the next offer.
    assert_eq!(
        code_of(&phone.ask(offer_of(8, 10 << 20)).await),
        ErrorCode::NoSpace
    );
}

#[tokio::test]
async fn a_desktop_with_files_off_refuses_them() {
    let downloads = tempfile::tempdir().unwrap();
    let (session, mut phone) = connect(Availability::Disabled, SessionConfig::default()).await;
    let _desktop = Driven::start(session, service(downloads.path()));

    assert_eq!(
        code_of(&phone.offer([1; 16], &[("a", b"x")]).await),
        ErrorCode::Disabled
    );
    let stray = chunk([1; 16], 0, 0, b"x", sha256(b"x"));
    assert_eq!(code_of(&phone.ask(stray).await), ErrorCode::Disabled);
}

#[tokio::test]
async fn transfers_from_the_phone_resume_after_a_lost_connection() {
    let downloads = tempfile::tempdir().unwrap();
    let a = data(600 * KIB, 3);
    let b = data(10, 4);
    let id = [8; 16];
    let files: [(&str, &[u8]); 2] = [("a.bin", &a), ("b.bin", &b)];

    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    assert_eq!(phone.offer(id, &files).await, accept(0, 0));
    let first = chunk(id, 0, 0, &a[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(first).await));
    drop(phone);
    desktop
        .reaches(TransferId(id), TransferState::Interrupted)
        .await;
    let service = desktop.stopped().await;
    assert_eq!(
        names_in(downloads.path()).len(),
        1,
        "the partial file stays"
    );

    let (mut desktop, mut phone) = desktop_with(service).await;
    // Chunks wait for the offer that resumes the transfer.
    let early = chunk(id, 0, CHUNK_LEN, &a[CHUNK_LEN..2 * CHUNK_LEN], Vec::new());
    assert_eq!(code_of(&phone.ask(early).await), ErrorCode::NotFound);
    let other = phone.offer(id, &[("a.bin", &a)]).await;
    assert_eq!(code_of(&other), ErrorCode::InvalidArgument);
    assert_eq!(phone.offer(id, &files).await, accept(0, CHUNK_LEN as u64));
    let answers = phone.send_file(id, 0, &a, CHUNK_LEN).await;
    assert!(answers.iter().all(is_ack), "{answers:?}");
    assert!(phone.send_file(id, 1, &b, 0).await.iter().all(is_ack));
    let done = desktop
        .reaches(TransferId(id), TransferState::Completed)
        .await;
    assert_eq!(fs::read(done.files[0].path.as_ref().unwrap()).unwrap(), a);
    assert_eq!(fs::read(done.files[1].path.as_ref().unwrap()).unwrap(), b);

    // Offered again, it is already complete, and takes no more chunks.
    assert_eq!(phone.offer(id, &files).await, accept(2, 0));
    let late = chunk(id, 1, 0, &b, sha256(&b));
    assert_eq!(code_of(&phone.ask(late).await), ErrorCode::NotFound);
    assert_eq!(names_in(downloads.path()), ["a.bin", "b.bin"]);
}

#[tokio::test]
async fn cancelling_a_transfer_from_the_phone_keeps_only_finished_files() {
    let downloads = tempfile::tempdir().unwrap();
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let small = data(10, 5);
    let big = data(600 * KIB, 6);
    let files: [(&str, &[u8]); 2] = [("small", &small), ("big", &big)];

    // The phone cancels.
    assert_eq!(phone.offer([1; 16], &files).await, accept(0, 0));
    assert!(
        phone
            .send_file([1; 16], 0, &small, 0)
            .await
            .iter()
            .all(is_ack)
    );
    let part = chunk([1; 16], 1, 0, &big[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(part).await));
    let cancel = |id: u8| {
        Payload::FileCancel(FileCancel {
            transfer_id: vec![id; 16],
        })
    };
    assert!(is_ack(&phone.ask(cancel(1)).await));
    desktop
        .reaches(
            TransferId([1; 16]),
            TransferState::Cancelled { by_peer: true },
        )
        .await;
    assert_eq!(names_in(downloads.path()), ["small"]);
    let late = chunk(
        [1; 16],
        1,
        CHUNK_LEN,
        &big[CHUNK_LEN..2 * CHUNK_LEN],
        Vec::new(),
    );
    assert_eq!(code_of(&phone.ask(late).await), ErrorCode::NotFound);
    assert!(
        is_ack(&phone.ask(cancel(9)).await),
        "unknown transfers are acked"
    );

    // The user cancels on the desktop.
    assert_eq!(phone.offer([2; 16], &[("big", &big)]).await, accept(0, 0));
    let part = chunk([2; 16], 0, 0, &big[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(part).await));
    desktop.cancel(TransferId([2; 16]));
    let request = phone.next().await;
    assert!(request.expects_reply);
    assert_eq!(request.payload, Some(cancel(2)));
    phone.ack(request.seq).await;
    desktop
        .reaches(
            TransferId([2; 16]),
            TransferState::Cancelled { by_peer: false },
        )
        .await;
    assert_eq!(names_in(downloads.path()), ["small"]);
    // Offered again, it is declined.
    let again = phone.offer([2; 16], &[("big", &big)]).await;
    assert_eq!(code_of(&again), ErrorCode::Declined);
}

#[tokio::test]
async fn kept_transfers_count_against_the_room_and_a_resume_is_never_busy() {
    let downloads = tempfile::tempdir().unwrap();
    let free = 100 * 1024 * 1024;
    let service = service(downloads.path()).with_free_space(move |_| Ok(free));
    let (desktop, mut phone) = desktop_with(service).await;
    let size = |id: u8, size: u64| {
        Payload::FileOffer(FileOffer {
            transfer_id: vec![id; 16],
            files: vec![FileInfo {
                name: "big".into(),
                size,
                ..FileInfo::default()
            }],
            ..FileOffer::default()
        })
    };
    assert_eq!(phone.ask(size(1, 30 << 20)).await, accept(0, 0));
    drop(phone);
    let service = desktop.stopped().await;

    // The 30 MiB kept for the phone count: 10 more do not fit.
    let (_desktop, mut phone) = desktop_with(service).await;
    assert_eq!(
        code_of(&phone.ask(size(2, 10 << 20)).await),
        ErrorCode::NoSpace
    );
    for n in 3..=6 {
        assert_eq!(phone.ask(size(n, 1024)).await, accept(0, 0));
    }
    assert_eq!(code_of(&phone.ask(size(7, 1024)).await), ErrorCode::Busy);
    // Four are running, but the kept one goes on.
    assert_eq!(phone.ask(size(1, 30 << 20)).await, accept(0, 0));
}

#[tokio::test]
async fn turning_files_off_ends_what_the_phone_sends() {
    let downloads = tempfile::tempdir().unwrap();
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let big = data(600 * KIB, 18);
    let files: [(&str, &[u8]); 1] = [("big", &big)];
    assert_eq!(phone.offer([1; 16], &files).await, accept(0, 0));
    let first = chunk([1; 16], 0, 0, &big[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(first).await));

    desktop.switch(Availability::Disabled);
    let status = phone.next().await;
    assert!(
        matches!(status.payload, Some(Payload::FeatureStatus(_))),
        "{status:?}"
    );
    desktop
        .reaches(
            TransferId([1; 16]),
            TransferState::Cancelled { by_peer: false },
        )
        .await;
    assert_eq!(names_in(downloads.path()), Vec::<String>::new());
    let next = chunk(
        [1; 16],
        0,
        CHUNK_LEN,
        &big[CHUNK_LEN..2 * CHUNK_LEN],
        Vec::new(),
    );
    assert_eq!(code_of(&phone.ask(next).await), ErrorCode::Disabled);

    // Back on, the phone cannot take it up again.
    desktop.switch(Availability::Ready);
    let status = phone.next().await;
    assert!(
        matches!(status.payload, Some(Payload::FeatureStatus(_))),
        "{status:?}"
    );
    assert_eq!(
        code_of(&phone.offer([1; 16], &files).await),
        ErrorCode::Declined
    );
}

#[tokio::test]
async fn a_cancel_made_while_the_phone_is_away_reaches_it_in_the_next_session() {
    let downloads = tempfile::tempdir().unwrap();
    let (desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let big = data(600 * KIB, 19);
    assert_eq!(phone.offer([1; 16], &[("big", &big)]).await, accept(0, 0));
    let first = chunk([1; 16], 0, 0, &big[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(first).await));
    drop(phone);
    let mut service = desktop.stopped().await;

    assert!(service.cancel(TransferId([1; 16])).await);
    assert_eq!(names_in(downloads.path()), Vec::<String>::new());
    let (_desktop, mut phone) = desktop_with(service).await;
    let cancel = phone.next().await;
    assert!(cancel.expects_reply);
    assert_eq!(
        cancel.payload,
        Some(Payload::FileCancel(FileCancel {
            transfer_id: vec![1; 16]
        }))
    );
    phone.ack(cancel.seq).await;
}

#[tokio::test]
async fn unfinished_transfers_from_the_phone_expire_after_an_hour() {
    let downloads = tempfile::tempdir().unwrap();
    let (desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let big = data(300 * KIB, 7);
    assert_eq!(phone.offer([1; 16], &[("big", &big)]).await, accept(0, 0));
    let part = chunk([1; 16], 0, 0, &big[..CHUNK_LEN], Vec::new());
    assert!(is_ack(&phone.ask(part).await));
    drop(phone);
    let mut service = desktop.stopped().await;

    let deadline = service.next_deadline().unwrap();
    assert!(deadline > Instant::now() + KEEP_UNFINISHED - Duration::from_secs(60));
    service.expire(deadline - Duration::from_secs(1)).await;
    assert_eq!(names_in(downloads.path()).len(), 1);
    service.expire(deadline).await;
    assert_eq!(names_in(downloads.path()), Vec::<String>::new());
    assert_eq!(
        service.transfers()[0].state,
        TransferState::Failed(Failure::Expired)
    );
    assert_eq!(service.next_deadline(), None);
}

#[test]
fn leftovers_of_an_earlier_run_are_removed() {
    let downloads = tempfile::tempdir().unwrap();
    let leftover = format!(".link2linux-{}-0.part", "ab".repeat(16));
    fs::write(downloads.path().join(&leftover), b"partial").unwrap();
    fs::write(downloads.path().join("mine.txt"), b"mine").unwrap();
    let removed = link2linux_core::files::remove_leftovers(downloads.path()).unwrap();
    assert_eq!(removed, 1);
    assert_eq!(names_in(downloads.path()), ["mine.txt"]);
}

/// Reads the desktop's offer and checks it names `id`, when given.
async fn read_offer(phone: &mut Phone) -> (u64, FileOffer) {
    let frame = phone.next().await;
    assert!(frame.expects_reply);
    match frame.payload {
        Some(Payload::FileOffer(offer)) => (frame.seq, offer),
        other => panic!("expected FileOffer, got {other:?}"),
    }
}

async fn read_chunk(phone: &mut Phone) -> (u64, FileChunk) {
    let frame = phone.next().await;
    assert!(frame.expects_reply);
    match frame.payload {
        Some(Payload::FileChunk(chunk)) => (frame.seq, chunk),
        other => panic!("expected FileChunk, got {other:?}"),
    }
}

#[tokio::test]
async fn files_go_to_the_phone_in_order_with_their_hashes() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let big = data(600 * KIB, 8);
    let paths = vec![
        source(sources.path(), "big.bin", &big),
        source(sources.path(), "empty.txt", b""),
        source(sources.path(), "small.txt", b"hello"),
    ];
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let id = desktop.send(paths.clone()).await.unwrap();

    let (seq, offer) = read_offer(&mut phone).await;
    assert_eq!(offer.transfer_id, id.0);
    let named: Vec<(&str, u64, &str)> = offer
        .files
        .iter()
        .map(|f| (f.name.as_str(), f.size, f.mime_type.as_str()))
        .collect();
    assert_eq!(
        named,
        [
            ("big.bin", big.len() as u64, ""),
            ("empty.txt", 0, "text/plain"),
            ("small.txt", 5, "text/plain")
        ]
    );
    assert!(offer.files.iter().all(|f| f.modified_unix_ms > 0));
    phone.answer(seq, accept(0, 0)).await;

    let mut received = vec![Vec::new(); 3];
    let mut hashes = Vec::new();
    let (mut index, mut at) = (0, 0_u64);
    while index < 3 {
        let (seq, chunk) = read_chunk(&mut phone).await;
        assert_eq!((chunk.file_index as usize, chunk.offset), (index, at));
        assert!(chunk.data.len() <= CHUNK_LEN);
        received[index].extend_from_slice(&chunk.data);
        at += chunk.data.len() as u64;
        if at == offer.files[index].size {
            hashes.push(chunk.sha256);
            index += 1;
            at = 0;
        } else {
            assert!(chunk.sha256.is_empty());
            assert!(!chunk.data.is_empty());
        }
        phone.ack(seq).await;
    }
    assert_eq!(received, [big.clone(), Vec::new(), b"hello".to_vec()]);
    assert_eq!(hashes, [sha256(&big), sha256(b""), sha256(b"hello")]);

    let done = desktop.reaches(id, TransferState::Completed).await;
    assert_eq!(
        (done.direction, done.done, done.size),
        (Direction::Outgoing, done.size, 600 * 1024 + 5)
    );
    assert_eq!(done.files[0].path.as_deref(), Some(paths[0].as_path()));
    phone.quiet().await;
}

#[tokio::test]
async fn at_most_eight_chunks_wait_for_answers_across_transfers() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let one = data(9 * CHUNK_LEN, 9);
    let two = data(9 * CHUNK_LEN, 10);
    let (desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let first = desktop
        .send(vec![source(sources.path(), "one", &one)])
        .await
        .unwrap();
    let second = desktop
        .send(vec![source(sources.path(), "two", &two)])
        .await
        .unwrap();

    for _ in 0..2 {
        let (seq, _) = read_offer(&mut phone).await;
        phone.answer(seq, accept(0, 0)).await;
    }
    let mut waiting = Vec::new();
    for _ in 0..8 {
        waiting.push(read_chunk(&mut phone).await);
    }
    phone.quiet().await;

    // The transfer with fewer chunks out gets the next free one, so both
    // share the session.
    let (seq, _) = waiting.remove(0);
    phone.ack(seq).await;
    let next = read_chunk(&mut phone).await;
    let fewer = if waiting
        .iter()
        .filter(|(_, c)| c.transfer_id == first.0)
        .count()
        > 4
    {
        second
    } else {
        first
    };
    assert_eq!(next.1.transfer_id, fewer.0);
    waiting.push(next);
    phone.quiet().await;

    let mut sent = [Vec::new(), Vec::new()];
    let mut ends = 0;
    while ends < 2 {
        let (seq, chunk) = if waiting.is_empty() {
            read_chunk(&mut phone).await
        } else {
            waiting.remove(0)
        };
        let which = usize::from(chunk.transfer_id != first.0);
        sent[which].push((chunk.offset, chunk.data.len()));
        if !chunk.sha256.is_empty() {
            let data = if which == 0 { &one } else { &two };
            assert_eq!(chunk.sha256, sha256(data));
            ends += 1;
        }
        phone.ack(seq).await;
    }
    desktop.settles(first, TransferState::Completed).await;
    desktop.settles(second, TransferState::Completed).await;
}

#[tokio::test]
async fn a_transfer_goes_on_where_the_phone_stands() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let file = data(5 * CHUNK_LEN, 11);
    let path = source(sources.path(), "file", &file);
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let id = desktop.send(vec![path.clone()]).await.unwrap();

    // The phone writes three chunks but answers two before the connection
    // drops.
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 0)).await;
    for n in 0..5 {
        let (seq, _) = read_chunk(&mut phone).await;
        if n < 2 {
            phone.ack(seq).await;
        }
    }
    drop(phone);
    desktop.reaches(id, TransferState::Interrupted).await;
    let service = desktop.stopped().await;
    assert_eq!(service.transfers()[0].done, 2 * CHUNK_LEN as u64);

    // Offered again in the next session, it goes on from the third chunk.
    let (mut desktop, mut phone) = desktop_with(service).await;
    let (seq, offer) = read_offer(&mut phone).await;
    assert_eq!(offer.transfer_id, id.0);
    phone.answer(seq, accept(0, 3 * CHUNK_LEN as u64)).await;
    let mut rest = Vec::new();
    loop {
        let (seq, chunk) = read_chunk(&mut phone).await;
        assert_eq!(chunk.offset as usize, 3 * CHUNK_LEN + rest.len());
        rest.extend_from_slice(&chunk.data);
        phone.ack(seq).await;
        if !chunk.sha256.is_empty() {
            assert_eq!(
                chunk.sha256,
                sha256(&file),
                "the hash covers the whole file"
            );
            break;
        }
    }
    assert_eq!(rest, file[3 * CHUNK_LEN..]);
    desktop.reaches(id, TransferState::Completed).await;

    // A position that is no chunk's end: the bytes before are hashed again.
    let second = desktop.send(vec![path.clone()]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 100_000)).await;
    let mut offset = 100_000;
    loop {
        let (seq, chunk) = read_chunk(&mut phone).await;
        assert_eq!(chunk.offset, offset);
        offset += chunk.data.len() as u64;
        phone.ack(seq).await;
        if !chunk.sha256.is_empty() {
            assert_eq!(chunk.sha256, sha256(&file));
            break;
        }
    }
    desktop.reaches(second, TransferState::Completed).await;

    // The phone already has it all.
    let third = desktop.send(vec![path]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(1, 0)).await;
    let done = desktop.reaches(third, TransferState::Completed).await;
    assert_eq!(done.done, file.len() as u64);
    phone.quiet().await;
}

#[tokio::test]
async fn refusals_end_or_delay_transfers() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let file = data(3 * CHUNK_LEN, 12);
    let path = source(sources.path(), "file", &file);
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;

    // A busy phone gets the same offer again later.
    let busy = desktop.send(vec![path.clone()]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    let wait = ErrorReply {
        code: ErrorCode::Busy.into(),
        retry_after_ms: 50,
        ..ErrorReply::default()
    };
    phone.answer(seq, Payload::ErrorReply(wait)).await;
    desktop.reaches(busy, TransferState::Waiting).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    desktop.transfers().await;
    let (seq, offer) = read_offer(&mut phone).await;
    assert_eq!(offer.transfer_id, busy.0);
    // A phone that failed gets it again a second later.
    phone.answer(seq, refusal(ErrorCode::Internal)).await;
    desktop.reaches(busy, TransferState::Waiting).await;
    phone.quiet().await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    desktop.transfers().await;
    let (seq, offer) = read_offer(&mut phone).await;
    assert_eq!(offer.transfer_id, busy.0);
    phone.answer(seq, refusal(ErrorCode::Disabled)).await;
    desktop
        .reaches(busy, TransferState::Failed(Failure::Disabled))
        .await;

    // The phone's user cancelled it earlier.
    let declined = desktop.send(vec![path.clone()]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, refusal(ErrorCode::Declined)).await;
    desktop
        .reaches(declined, TransferState::Cancelled { by_peer: true })
        .await;

    let full = desktop.send(vec![path.clone()]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, refusal(ErrorCode::NoSpace)).await;
    desktop
        .reaches(full, TransferState::Failed(Failure::NoSpace))
        .await;

    // A refused chunk ends the transfer; the answers to the others do not
    // count.
    let lost = desktop.send(vec![path]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 0)).await;
    let chunks = [
        read_chunk(&mut phone).await,
        read_chunk(&mut phone).await,
        read_chunk(&mut phone).await,
    ];
    phone
        .answer(chunks[0].0, refusal(ErrorCode::NotFound))
        .await;
    desktop
        .reaches(lost, TransferState::Failed(Failure::Refused))
        .await;
    phone.ack(chunks[1].0).await;
    phone.ack(chunks[2].0).await;
    phone.quiet().await;
    assert_eq!(
        desktop.transfer(lost).await.state,
        TransferState::Failed(Failure::Refused)
    );
}

#[tokio::test]
async fn folders_and_missing_files_are_not_sent() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let (desktop, mut phone) = desktop_with(service(downloads.path())).await;

    let folder = desktop.send(vec![sources.path().to_owned()]).await;
    assert!(matches!(folder, Err(SendError::Directory(_))), "{folder:?}");
    let missing = desktop.send(vec![sources.path().join("missing")]).await;
    assert!(
        matches!(missing, Err(SendError::Unreadable { .. })),
        "{missing:?}"
    );
    assert!(matches!(
        desktop.send(Vec::new()).await,
        Err(SendError::NoFiles)
    ));
    let many = vec![sources.path().join("x"); 501];
    assert!(matches!(desktop.send(many).await, Err(SendError::TooMany)));
    assert!(desktop.transfers().await.is_empty());
    phone.quiet().await;
}

#[tokio::test]
async fn a_file_that_changes_while_it_is_sent_cancels_the_transfer() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let file = data(12 * CHUNK_LEN, 13);
    let path = source(sources.path(), "file", &file);
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;
    let id = desktop.send(vec![path.clone()]).await.unwrap();

    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 0)).await;
    let mut waiting = Vec::new();
    for _ in 0..8 {
        waiting.push(read_chunk(&mut phone).await.0);
    }
    fs::write(&path, &file[..CHUNK_LEN]).unwrap();
    phone.ack(waiting[0]).await;

    let cancel = phone.next().await;
    assert!(cancel.expects_reply);
    assert_eq!(
        cancel.payload,
        Some(Payload::FileCancel(FileCancel {
            transfer_id: id.0.to_vec()
        }))
    );
    phone.ack(cancel.seq).await;
    desktop
        .reaches(id, TransferState::Failed(Failure::Changed))
        .await;
}

#[tokio::test]
async fn cancelling_stops_sending_on_either_side() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let file = data(12 * CHUNK_LEN, 14);
    let path = source(sources.path(), "file", &file);
    let (mut desktop, mut phone) = desktop_with(service(downloads.path())).await;

    // On the desktop.
    let mine = desktop.send(vec![path.clone()]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 0)).await;
    let mut waiting = Vec::new();
    for _ in 0..8 {
        waiting.push(read_chunk(&mut phone).await.0);
    }
    desktop.cancel(mine);
    let cancel = phone.next().await;
    assert!(matches!(cancel.payload, Some(Payload::FileCancel(_))));
    phone.ack(cancel.seq).await;
    desktop
        .reaches(mine, TransferState::Cancelled { by_peer: false })
        .await;
    for seq in waiting {
        phone.answer(seq, refusal(ErrorCode::NotFound)).await;
    }
    phone.quiet().await;

    // On the phone.
    let theirs = desktop.send(vec![path]).await.unwrap();
    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 0)).await;
    let mut waiting = Vec::new();
    for _ in 0..8 {
        waiting.push(read_chunk(&mut phone).await.0);
    }
    let cancel = Payload::FileCancel(FileCancel {
        transfer_id: theirs.0.to_vec(),
    });
    assert!(is_ack(&phone.ask(cancel).await));
    desktop
        .reaches(theirs, TransferState::Cancelled { by_peer: true })
        .await;
    for seq in waiting {
        phone.ack(seq).await;
    }
    phone.quiet().await;
    assert_eq!(
        desktop.transfer(theirs).await.state,
        TransferState::Cancelled { by_peer: true }
    );
}

#[tokio::test]
async fn unanswered_requests_offer_the_transfer_again_then_wait_for_the_next_session() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let file = data(CHUNK_LEN + 10, 15);
    let path = source(sources.path(), "file", &file);
    let config = SessionConfig {
        request_timeout: Duration::from_millis(200),
        ..SessionConfig::default()
    };
    let (session, mut phone) = connect(Availability::Ready, config).await;
    let mut desktop = Driven::start(session, service(downloads.path()));
    let id = desktop.send(vec![path]).await.unwrap();

    let (seq, _) = read_offer(&mut phone).await;
    phone.answer(seq, accept(0, 0)).await;
    read_chunk(&mut phone).await;
    read_chunk(&mut phone).await;
    // Neither chunk is answered, then neither offer.
    for _ in 0..2 {
        let (_, offer) = read_offer(&mut phone).await;
        assert_eq!(offer.transfer_id, id.0);
    }
    // Three missed: nothing more in this session, and no cancel.
    desktop.reaches(id, TransferState::Interrupted).await;
    phone.quiet().await;
    drop(phone);
    let service = desktop.stopped().await;

    // The next session goes on with it.
    let (desktop, mut phone) = desktop_with(service).await;
    let (seq, offer) = read_offer(&mut phone).await;
    assert_eq!(offer.transfer_id, id.0);
    phone.answer(seq, accept(1, 0)).await;
    desktop.settles(id, TransferState::Completed).await;
}

#[tokio::test]
async fn chunks_that_got_no_answer_keep_their_place_in_the_window() {
    let sources = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let config = SessionConfig {
        request_timeout: Duration::from_millis(200),
        ..SessionConfig::default()
    };
    let (session, mut phone) = connect(Availability::Ready, config).await;
    let desktop = Driven::start(session, service(downloads.path()));
    let one = desktop
        .send(vec![source(
            sources.path(),
            "one",
            &data(12 * CHUNK_LEN, 20),
        )])
        .await
        .unwrap();
    let two = desktop
        .send(vec![source(
            sources.path(),
            "two",
            &data(12 * CHUNK_LEN, 21),
        )])
        .await
        .unwrap();
    for _ in 0..2 {
        let (seq, _) = read_offer(&mut phone).await;
        phone.answer(seq, accept(0, 0)).await;
    }
    let mut stalled = Vec::new();
    for _ in 0..8 {
        let (seq, chunk) = read_chunk(&mut phone).await;
        stalled.push((seq, chunk.transfer_id));
    }

    // The phone stalls past the timeout. Each transfer whose chunk missed
    // its answer is offered again, but the phone may still hold all eight
    // chunks: no ninth comes.
    let mut stalled_transfers: Vec<&Vec<u8>> = stalled.iter().map(|(_, id)| id).collect();
    stalled_transfers.dedup();
    stalled_transfers.sort();
    stalled_transfers.dedup();
    let mut offers = Vec::new();
    while offers.len() < stalled_transfers.len() {
        let frame = phone.next().await;
        match frame.payload {
            Some(Payload::FileOffer(offer)) => offers.push((frame.seq, offer.transfer_id)),
            other => panic!("expected only offers, got {other:?}"),
        }
    }

    // It answers in order: the stalled chunks, too late for the desktop,
    // then the offers. Only then the window opens again, up to eight.
    for (seq, _) in &stalled {
        phone.ack(*seq).await;
    }
    for (seq, id) in &offers {
        let written = stalled.iter().filter(|(_, of)| of == id).count();
        phone
            .answer(*seq, accept(0, (written * CHUNK_LEN) as u64))
            .await;
    }
    let mut sent = Vec::new();
    for _ in 0..8 {
        sent.push(read_chunk(&mut phone).await.1.transfer_id);
    }
    // Unanswered again, they bring offers, never a ninth chunk.
    let next = phone.next().await;
    assert!(
        matches!(next.payload, Some(Payload::FileOffer(_))),
        "{next:?}"
    );
    assert!(sent.contains(&two.0.to_vec()), "both transfers go on");
    assert!(sent.contains(&one.0.to_vec()), "both transfers go on");
}

/// A phone running the same service, on the other end of a pipe.
async fn pair(desktop_dir: &Path, phone_dir: &Path) -> (Driven, Driven) {
    let (here, there) = duplex(64 * 1024);
    let (desktop_session, phone_session) = tokio::join!(
        Session::handshake(here, desktop(), SessionConfig::default()),
        Session::handshake(there, phone(), SessionConfig::phone()),
    );
    (
        Driven::start(desktop_session.unwrap(), service(desktop_dir)),
        Driven::start(phone_session.unwrap(), service(phone_dir)),
    )
}

#[tokio::test]
async fn two_services_exchange_files_both_ways_at_once() {
    let sources = tempfile::tempdir().unwrap();
    let on_desktop = tempfile::tempdir().unwrap();
    let on_phone = tempfile::tempdir().unwrap();
    let to_phone = data(2 * 1024 * KIB + 123, 16);
    let to_desktop = data(2 * 1024 * KIB + 321, 17);
    let up = source(sources.path(), "to phone.bin", &to_phone);
    let down = source(sources.path(), "to desktop.bin", &to_desktop);
    let (desktop, phone) = pair(on_desktop.path(), on_phone.path()).await;

    let (sent_up, sent_down) = tokio::join!(desktop.send(vec![up]), phone.send(vec![down]));
    let (up_id, down_id) = (sent_up.unwrap(), sent_down.unwrap());
    for (side, id) in [
        (&desktop, down_id),
        (&phone, up_id),
        (&desktop, up_id),
        (&phone, down_id),
    ] {
        side.settles(id, TransferState::Completed).await;
    }
    assert_eq!(
        desktop.transfer(down_id).await.direction,
        Direction::Incoming
    );
    assert_eq!(phone.transfer(up_id).await.direction, Direction::Incoming);
    assert_eq!(
        fs::read(on_desktop.path().join("to desktop.bin")).unwrap(),
        to_desktop
    );
    assert_eq!(
        fs::read(on_phone.path().join("to phone.bin")).unwrap(),
        to_phone
    );
}

#[test]
fn files_are_checked_before_they_are_sent() {
    use link2linux_core::files::check_sendable;

    let sources = tempfile::tempdir().unwrap();
    let file = source(sources.path(), "a.txt", b"a");
    assert!(check_sendable(std::slice::from_ref(&file)).is_ok());
    let folder = sources.path().to_owned();
    assert!(matches!(
        check_sendable(&[file.clone(), folder]),
        Err(SendError::Directory(_))
    ));
    assert!(matches!(
        check_sendable(&[sources.path().join("missing")]),
        Err(SendError::Unreadable { .. })
    ));
    assert!(matches!(check_sendable(&[]), Err(SendError::NoFiles)));
    assert!(matches!(
        check_sendable(&vec![file; 501]),
        Err(SendError::TooMany)
    ));
}
