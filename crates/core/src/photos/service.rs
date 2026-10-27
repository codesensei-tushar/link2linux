//! [`PhotoService`]: the photos feature with one phone.

use std::collections::{HashMap, HashSet, VecDeque};
use std::mem;

use link2linux_protocol::Payload;
use link2linux_protocol::v1::{
    self as wire, ErrorCode, ErrorReply, PhotoDataRequest, PhotoListRequest, PhotoThumbnailRequest,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio::time::{Duration, Instant};

use super::cache::{ThumbnailCache, ThumbnailKey};
use super::{
    CHUNK_BYTES, MAX_PHOTO_BYTES, MAX_PHOTOS, PhotoError, PhotoFile, PhotoInfo, PhotosState,
    THUMBNAIL_SIZES, Thumbnail,
};
use crate::Feature;
use crate::session::{Availability, Session, SessionError, SessionEvent};

/// Where a thumbnail goes once it arrives, or why it does not.
pub type ThumbnailWaiter = oneshot::Sender<Result<Thumbnail, PhotoError>>;

/// Where a photo goes once all of it arrived, or why it does not.
pub type PhotoWaiter = oneshot::Sender<Result<PhotoFile, PhotoError>>;

/// Thumbnail requests the desktop keeps unanswered per session.
const MAX_THUMBNAILS_ASKED: usize = 8;

/// Data requests the desktop keeps unanswered per session.
const MAX_DATA_ASKED: usize = 4;

/// The image data the thumbnail cache holds.
const CACHE_BYTES: usize = 64 * 1024 * 1024;

/// How long to wait after `BUSY` without a `retry_after_ms`.
const BUSY_WAIT: Duration = Duration::from_secs(1);

/// How long the desktop waits before it asks for the list again after a
/// request that timed out or failed in a way that may pass: for the first
/// retry, the second, and every one after.
const RETRY_AFTER: [Duration; 3] = [
    Duration::from_secs(5),
    Duration::from_secs(30),
    Duration::from_secs(300),
];

/// A thumbnail someone waits for.
#[derive(Debug)]
struct WantedThumbnail {
    key: ThumbnailKey,
    waiters: Vec<ThumbnailWaiter>,
}

/// A photo being fetched, from offset 0 on.
#[derive(Debug)]
struct Fetch {
    id: String,
    name: String,
    /// The file's size and modified time, from the first answer.
    file: Option<(u64, i64)>,
    data: Vec<u8>,
    /// The next offset to ask for.
    next: u64,
    received: u64,
    /// The photo changed once while it was fetched, and the fetch started
    /// again from offset 0: a second change fails it.
    restarted: bool,
    /// Counts the starts, so answers to requests of an earlier start are
    /// dropped.
    round: u32,
    /// Offsets to ask for again, after `BUSY`.
    again: Vec<u64>,
    waiters: Vec<PhotoWaiter>,
}

/// What an answer did to a fetch.
enum Progress {
    Going,
    Done,
    Failed(PhotoError),
}

/// A data request that waits for its answer.
#[derive(Debug)]
struct DataAsked {
    id: String,
    round: u32,
    offset: u64,
}

/// The photos feature with one phone, driven by its session. It does not
/// own the session, so it can share one with other features:
///
/// 1. [`PhotoService::session_started`] once the session is up;
/// 2. every [`SessionEvent`] through [`PhotoService::handle_event`], which
///    keeps the photo events and hands back the rest;
/// 3. [`PhotoService::thumbnail`] and [`PhotoService::fetch`] when the user's
///    view needs a photo, and [`PhotoService::tick`] at
///    [`PhotoService::next_deadline`];
/// 4. [`PhotoService::session_ended`] when the session is gone.
///
/// [`PhotoService::state`] and [`PhotoService::photos`] say what to show
/// after each of them.
///
/// It keeps at most 8 thumbnail and 4 data requests unanswered, and asks
/// for the thumbnails wanted last first, since those are the ones the user
/// looks at now. A waiter whose receiver is gone is no longer asked for.
#[derive(Debug)]
pub struct PhotoService {
    state: PhotosState,
    /// Whether the feature was ready on both devices at the last look.
    ready: bool,
    /// The `PhotoListRequest` that waits for its answer.
    asked_list: Option<u64>,
    /// A list request was sent while the feature has been ready: the phone
    /// counts the desktop as subscribed, so its changes apply.
    following: bool,
    /// When to ask for the list again after a request that got no usable
    /// answer.
    retry_at: Option<Instant>,
    /// The list requests in a row that got no usable answer.
    failures: usize,
    photos: Vec<PhotoInfo>,
    cache: ThumbnailCache,
    /// Thumbnails not asked for yet, the newest wish last.
    wanted: VecDeque<WantedThumbnail>,
    thumbnails_asked: HashMap<u64, WantedThumbnail>,
    /// After `BUSY`, no thumbnail is asked for until then.
    thumbnails_paused: Option<Instant>,
    /// Photos being fetched, in the order they were asked for.
    fetches: Vec<Fetch>,
    data_asked: HashMap<u64, DataAsked>,
    /// After `BUSY`, no data is asked for until then.
    data_paused: Option<Instant>,
}

impl Default for PhotoService {
    fn default() -> Self {
        Self::new()
    }
}

impl PhotoService {
    /// A service with no session yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: PhotosState::Disconnected,
            ready: false,
            asked_list: None,
            following: false,
            retry_at: None,
            failures: 0,
            photos: Vec::new(),
            cache: ThumbnailCache::new(CACHE_BYTES),
            wanted: VecDeque::new(),
            thumbnails_asked: HashMap::new(),
            thumbnails_paused: None,
            fetches: Vec::new(),
            data_asked: HashMap::new(),
            data_paused: None,
        }
    }

    /// What to show of the phone's photos now.
    #[must_use]
    pub fn state(&self) -> PhotosState {
        self.state
    }

    /// The phone's photos, newest first, while the state is
    /// [`PhotosState::Listed`]; empty otherwise.
    #[must_use]
    pub fn photos(&self) -> &[PhotoInfo] {
        &self.photos
    }

    /// Asks for the list if the feature is ready on both devices. Call it
    /// once the session is up.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn session_started<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.ready = false;
        self.look(session).await
    }

    /// The session is gone: what waits fails, and the list is no longer
    /// current. The thumbnails stay for the next session with the phone.
    pub fn session_ended(&mut self) {
        self.ready = false;
        self.stop_following();
        self.fail_all(PhotoError::NotConnected);
        self.state = PhotosState::Disconnected;
    }

    /// Takes the events of the photos feature and returns every other
    /// event, for the caller to handle. A `FeatureStatus` is used and
    /// returned too.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn handle_event<S>(
        &mut self,
        session: &mut Session<S>,
        event: SessionEvent,
    ) -> Result<Option<SessionEvent>, SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        match event {
            SessionEvent::Reply(reply) if self.asked_list == Some(reply.request) => {
                self.asked_list = None;
                self.list_answered(reply.into_result());
                Ok(None)
            }
            SessionEvent::Reply(reply) if self.thumbnails_asked.contains_key(&reply.request) => {
                if let Some(wanted) = self.thumbnails_asked.remove(&reply.request) {
                    self.thumbnail_answered(wanted, reply.into_result());
                }
                self.pump(session, Instant::now())?;
                Ok(None)
            }
            SessionEvent::Reply(reply) if self.data_asked.contains_key(&reply.request) => {
                if let Some(asked) = self.data_asked.remove(&reply.request) {
                    self.data_answered(&asked, reply.into_result());
                }
                self.pump(session, Instant::now())?;
                Ok(None)
            }
            SessionEvent::RequestTimedOut { request } if self.asked_list == Some(request) => {
                self.asked_list = None;
                tracing::info!("the phone did not send its photos in time");
                self.state = PhotosState::Unavailable;
                self.retry_later(0);
                Ok(None)
            }
            SessionEvent::RequestTimedOut { request }
                if self.thumbnails_asked.contains_key(&request) =>
            {
                if let Some(wanted) = self.thumbnails_asked.remove(&request) {
                    tracing::debug!("a thumbnail did not come in time");
                    answer_all(wanted.waiters, &Err(PhotoError::TimedOut));
                }
                self.pump(session, Instant::now())?;
                Ok(None)
            }
            SessionEvent::RequestTimedOut { request } if self.data_asked.contains_key(&request) => {
                if let Some(asked) = self.data_asked.remove(&request) {
                    tracing::debug!("a photo's data did not come in time");
                    self.end_fetch(&asked.id, asked.round, Err(PhotoError::TimedOut));
                }
                self.pump(session, Instant::now())?;
                Ok(None)
            }
            SessionEvent::Message {
                payload: Payload::PhotoListChanged(changed),
                ..
            } => {
                if self.following {
                    self.set_list(&changed.photos);
                    // The subscription works after all.
                    self.retry_at = None;
                    self.failures = 0;
                    tracing::trace!(photos = self.photos.len(), "the phone's photos changed");
                } else {
                    tracing::debug!("ignoring a photo list the desktop did not ask for");
                }
                Ok(None)
            }
            SessionEvent::FeatureStatus(states) => {
                self.look(session).await?;
                Ok(Some(SessionEvent::FeatureStatus(states)))
            }
            other => Ok(Some(other)),
        }
    }

    /// Gets the thumbnail of photo `id` with its longest side at most
    /// `max_size` (held to 64 to 2048) for `waiter`: from memory when it
    /// is there, otherwise from the phone.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn thumbnail<S>(
        &mut self,
        session: &mut Session<S>,
        id: &str,
        max_size: u32,
        waiter: ThumbnailWaiter,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let key = match self.listed(id) {
            Ok(photo) => ThumbnailKey {
                id: photo.id.clone(),
                modified_unix_ms: photo.modified_unix_ms,
                max_size: max_size.clamp(*THUMBNAIL_SIZES.start(), *THUMBNAIL_SIZES.end()),
            },
            Err(error) => {
                drop(waiter.send(Err(error)));
                return Ok(());
            }
        };
        if let Some(thumbnail) = self.cache.get(&key) {
            drop(waiter.send(Ok(thumbnail)));
            return Ok(());
        }
        if let Some(asked) = self
            .thumbnails_asked
            .values_mut()
            .find(|asked| asked.key == key)
        {
            asked.waiters.push(waiter);
            return Ok(());
        }
        // Wanted again: it moves to the end, to be asked for first.
        let mut waiters = vec![waiter];
        if let Some(index) = self.wanted.iter().position(|wanted| wanted.key == key)
            && let Some(earlier) = self.wanted.remove(index)
        {
            waiters.extend(earlier.waiters);
        }
        self.wanted.push_back(WantedThumbnail { key, waiters });
        self.pump(session, Instant::now())
    }

    /// Fetches the original file of photo `id` for `waiter`.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub fn fetch<S>(
        &mut self,
        session: &mut Session<S>,
        id: &str,
        waiter: PhotoWaiter,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let photo = match self.listed(id) {
            Ok(photo) if photo.size > MAX_PHOTO_BYTES => Err(PhotoError::TooLarge),
            listed => listed,
        };
        let (id, name) = match photo {
            Ok(photo) => (photo.id.clone(), photo.name.clone()),
            Err(error) => {
                drop(waiter.send(Err(error)));
                return Ok(());
            }
        };
        if let Some(fetch) = self.fetches.iter_mut().find(|fetch| fetch.id == id) {
            fetch.waiters.push(waiter);
            return Ok(());
        }
        self.fetches.push(Fetch {
            id,
            name,
            file: None,
            data: Vec::new(),
            next: 0,
            received: 0,
            restarted: false,
            round: 0,
            again: Vec::new(),
            waiters: vec![waiter],
        });
        self.pump(session, Instant::now())
    }

    /// When the service next has something to do by itself: ask for the
    /// list again, or go on after `BUSY`.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        [self.retry_at, self.thumbnails_paused, self.data_paused]
            .into_iter()
            .flatten()
            .min()
    }

    /// The time [`PhotoService::next_deadline`] named may have come.
    ///
    /// # Errors
    ///
    /// Fails when the session fails.
    pub async fn tick<S>(
        &mut self,
        session: &mut Session<S>,
        now: Instant,
    ) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.retry_at.is_some_and(|at| at <= now) {
            self.retry_at = None;
            if self.ready && self.asked_list.is_none() && session.is_ready(Feature::Photos) {
                self.ask(session).await?;
            }
        }
        self.pump(session, now)
    }

    /// The listed photo `id`, or why there is none.
    fn listed(&self, id: &str) -> Result<&PhotoInfo, PhotoError> {
        match self.state {
            PhotosState::Listed => self
                .photos
                .iter()
                .find(|photo| photo.id == id)
                .ok_or(PhotoError::NotFound),
            PhotosState::Disconnected => Err(PhotoError::NotConnected),
            _ => Err(PhotoError::NotReady),
        }
    }

    /// Asks for the list when the feature just became ready on both
    /// devices, and lets go of everything when it no longer is.
    async fn look<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if !session.is_ready(Feature::Photos) {
            if self.ready {
                tracing::debug!("the phone's photos are no longer followed");
            }
            self.ready = false;
            self.stop_following();
            self.fail_all(PhotoError::NotReady);
            self.state = not_ready(session);
            // What the phone said it no longer shares goes too. Before its
            // first status, nothing is known yet.
            if session
                .peer_availability()
                .is_some_and(|states| states.get(&Feature::Photos) != Some(&Availability::Ready))
            {
                self.cache.clear();
            }
            return Ok(());
        }
        if !self.ready {
            self.failures = 0;
            self.retry_at = None;
            self.ask(session).await?;
            self.ready = true;
            self.state = PhotosState::Waiting;
        }
        Ok(())
    }

    async fn ask<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let seq = session
            .start_request(Payload::PhotoListRequest(PhotoListRequest {}))
            .await?;
        tracing::debug!(seq, "asked for the phone's photos");
        self.asked_list = Some(seq);
        self.following = true;
        Ok(())
    }

    /// Forgets the subscription and the list. A late answer to the list
    /// request that waits is not taken.
    fn stop_following(&mut self) {
        self.asked_list = None;
        self.following = false;
        self.retry_at = None;
        self.failures = 0;
        self.photos.clear();
    }

    /// Plans the next list request after one that got no usable answer: in
    /// `retry_after_ms` when the phone said, else after a backoff.
    fn retry_later(&mut self, retry_after_ms: u32) {
        let backoff = RETRY_AFTER[self.failures.min(RETRY_AFTER.len() - 1)];
        let wait = match retry_after_ms {
            0 => backoff,
            ms => Duration::from_millis(ms.into()),
        };
        self.failures += 1;
        self.retry_at = Some(Instant::now() + wait);
    }

    fn list_answered(&mut self, answer: Result<Payload, ErrorReply>) {
        self.state = match answer {
            Ok(Payload::PhotoListReply(reply)) => {
                self.following = true;
                self.retry_at = None;
                self.failures = 0;
                self.set_list(&reply.photos);
                tracing::debug!(photos = self.photos.len(), "the phone sent its photos");
                PhotosState::Listed
            }
            Ok(_) => {
                tracing::debug!("the phone answered the photo list request with something else");
                self.retry_later(0);
                PhotosState::Unavailable
            }
            Err(error) => {
                tracing::info!(code = error.code, "the phone did not send its photos");
                match error.code() {
                    ErrorCode::Disabled => PhotosState::TurnedOff,
                    ErrorCode::PermissionRequired => PhotosState::PermissionRequired,
                    ErrorCode::Unsupported => PhotosState::Unsupported,
                    ErrorCode::Unavailable | ErrorCode::Busy | ErrorCode::Internal => {
                        self.retry_later(error.retry_after_ms);
                        PhotosState::Unavailable
                    }
                    _ => PhotosState::Unavailable,
                }
            }
        };
    }

    /// Takes the phone's new list: photos whose id breaks the rules are
    /// left out, as is any id after its first. Thumbnails of photos no
    /// longer listed, or of an older version, go.
    fn set_list(&mut self, photos: &[wire::Photo]) {
        let mut ids = HashSet::new();
        self.photos = photos
            .iter()
            .filter_map(PhotoInfo::from_wire)
            .filter(|photo| ids.insert(photo.id.clone()))
            .take(MAX_PHOTOS)
            .collect();
        let versions: HashSet<(&str, i64)> = self
            .photos
            .iter()
            .map(|photo| (photo.id.as_str(), photo.modified_unix_ms))
            .collect();
        self.cache
            .retain(|key| versions.contains(&(key.id.as_str(), key.modified_unix_ms)));
    }

    fn thumbnail_answered(&mut self, wanted: WantedThumbnail, answer: Result<Payload, ErrorReply>) {
        let result = match answer {
            Ok(Payload::PhotoThumbnail(thumbnail)) => {
                Thumbnail::from_wire(thumbnail).ok_or(PhotoError::Failed)
            }
            Ok(_) => Err(PhotoError::Failed),
            Err(error) => match error.code() {
                ErrorCode::Busy | ErrorCode::Unavailable => {
                    // Asked for again, first, once the phone has time.
                    self.thumbnails_paused = Some(Instant::now() + busy_wait(&error));
                    self.wanted.push_back(wanted);
                    return;
                }
                code => Err(refused(code)),
            },
        };
        if let Err(error) = result {
            tracing::debug!(%error, "no thumbnail");
        }
        // Kept only for the version listed now.
        if let Ok(thumbnail) = &result
            && self.photos.iter().any(|photo| {
                photo.id == wanted.key.id && photo.modified_unix_ms == wanted.key.modified_unix_ms
            })
        {
            self.cache.insert(wanted.key, thumbnail.clone());
        }
        answer_all(wanted.waiters, &result);
    }

    fn data_answered(&mut self, asked: &DataAsked, answer: Result<Payload, ErrorReply>) {
        let Some(index) = self
            .fetches
            .iter()
            .position(|fetch| fetch.id == asked.id && fetch.round == asked.round)
        else {
            // For a fetch that ended or started again since.
            return;
        };
        let fetch = &mut self.fetches[index];
        let progress = match answer {
            Ok(Payload::PhotoData(data)) => fetch.take(asked.offset, data),
            Ok(_) => Progress::Failed(PhotoError::Failed),
            Err(error) => match error.code() {
                ErrorCode::Busy | ErrorCode::Unavailable => {
                    fetch.again.push(asked.offset);
                    self.data_paused = Some(Instant::now() + busy_wait(&error));
                    Progress::Going
                }
                code => Progress::Failed(refused(code)),
            },
        };
        let result = match progress {
            Progress::Going => return,
            Progress::Done => {
                let fetch = &mut self.fetches[index];
                Ok(PhotoFile {
                    name: mem::take(&mut fetch.name),
                    modified_unix_ms: fetch.file.map_or(0, |(_, modified)| modified),
                    data: mem::take(&mut fetch.data).into(),
                })
            }
            Progress::Failed(error) => {
                tracing::debug!(%error, "a photo could not be fetched");
                Err(error)
            }
        };
        let fetch = self.fetches.remove(index);
        answer_all(fetch.waiters, &result);
    }

    /// Ends the fetch of `id` in its start `round` with `result`.
    fn end_fetch(&mut self, id: &str, round: u32, result: Result<PhotoFile, PhotoError>) {
        if let Some(index) = self
            .fetches
            .iter()
            .position(|fetch| fetch.id == id && fetch.round == round)
        {
            let fetch = self.fetches.remove(index);
            answer_all(fetch.waiters, &result);
        }
    }

    /// Asks for what waits, within the limits on unanswered requests.
    fn pump<S>(&mut self, session: &mut Session<S>, now: Instant) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if self.state != PhotosState::Listed {
            return Ok(());
        }
        if self.thumbnails_paused.is_some_and(|until| until > now) {
            // Still waiting after BUSY.
        } else {
            self.thumbnails_paused = None;
            self.ask_thumbnails(session)?;
        }
        if self.data_paused.is_some_and(|until| until > now) {
            // Still waiting after BUSY.
        } else {
            self.data_paused = None;
            self.ask_data(session)?;
        }
        session.write_queued()
    }

    fn ask_thumbnails<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        while self.thumbnails_asked.len() < MAX_THUMBNAILS_ASKED {
            let Some(mut wanted) = self.wanted.pop_back() else {
                break;
            };
            wanted.waiters.retain(|waiter| !waiter.is_closed());
            if wanted.waiters.is_empty() {
                continue;
            }
            let request = Payload::PhotoThumbnailRequest(PhotoThumbnailRequest {
                id: wanted.key.id.clone(),
                max_size: wanted.key.max_size,
            });
            match session.queue_request(request) {
                Ok(seq) => {
                    self.thumbnails_asked.insert(seq, wanted);
                }
                Err(error) => {
                    answer_all(wanted.waiters, &Err(PhotoError::NotReady));
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn ask_data<S>(&mut self, session: &mut Session<S>) -> Result<(), SessionError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        // Nobody waits for these any more.
        self.fetches.retain_mut(|fetch| {
            fetch.waiters.retain(|waiter| !waiter.is_closed());
            !fetch.waiters.is_empty()
        });
        for fetch in &mut self.fetches {
            while self.data_asked.len() < MAX_DATA_ASKED {
                let Some(offset) = fetch.next_offset() else {
                    break;
                };
                let request = Payload::PhotoDataRequest(PhotoDataRequest {
                    id: fetch.id.clone(),
                    offset,
                });
                let seq = session.queue_request(request)?;
                self.data_asked.insert(
                    seq,
                    DataAsked {
                        id: fetch.id.clone(),
                        round: fetch.round,
                        offset,
                    },
                );
            }
        }
        Ok(())
    }

    /// Everything that waits fails with `error`. Answers to the requests
    /// already sent are not taken.
    fn fail_all(&mut self, error: PhotoError) {
        let result = Err(error);
        for wanted in self
            .wanted
            .drain(..)
            .chain(self.thumbnails_asked.drain().map(|(_, wanted)| wanted))
        {
            answer_all(wanted.waiters, &result);
        }
        for fetch in self.fetches.drain(..) {
            answer_all(fetch.waiters, &Err(error));
        }
        self.data_asked.clear();
        self.thumbnails_paused = None;
        self.data_paused = None;
    }
}

impl Fetch {
    /// The next offset to ask for: 0 first, the rest once the first answer
    /// gave the file's size.
    fn next_offset(&mut self) -> Option<u64> {
        if let Some(offset) = self.again.pop() {
            return Some(offset);
        }
        match self.file {
            None if self.next == 0 => {
                self.next = CHUNK_BYTES;
                Some(0)
            }
            Some((size, _)) if self.next < size => {
                let offset = self.next;
                self.next += CHUNK_BYTES;
                Some(offset)
            }
            _ => None,
        }
    }

    /// Takes the answer for `offset`.
    fn take(&mut self, offset: u64, data: wire::PhotoData) -> Progress {
        match self.file {
            None => {
                if data.size > MAX_PHOTO_BYTES {
                    return Progress::Failed(PhotoError::TooLarge);
                }
                self.file = Some((data.size, data.modified_unix_ms));
                // Within MAX_PHOTO_BYTES, which fits in memory.
                self.data = vec![0; usize::try_from(data.size).unwrap_or(0)];
            }
            Some((size, modified)) if size != data.size || modified != data.modified_unix_ms => {
                if self.restarted {
                    return Progress::Failed(PhotoError::Changed);
                }
                tracing::debug!("a photo changed while it was fetched; starting again");
                self.restarted = true;
                self.round += 1;
                self.file = None;
                self.data = Vec::new();
                self.next = 0;
                self.received = 0;
                self.again.clear();
                return Progress::Going;
            }
            Some(_) => {}
        }
        let size = data.size;
        // 256 KiB from the offset, or fewer where the file ends first.
        let expected = size.saturating_sub(offset).min(CHUNK_BYTES);
        if offset > size || data.data.len() as u64 != expected {
            return Progress::Failed(PhotoError::Failed);
        }
        let (Ok(start), Ok(len)) = (usize::try_from(offset), usize::try_from(expected)) else {
            return Progress::Failed(PhotoError::TooLarge);
        };
        self.data[start..start + len].copy_from_slice(&data.data);
        self.received += expected;
        if self.received >= size {
            Progress::Done
        } else {
            Progress::Going
        }
    }
}

/// How long to wait after `error`, a `BUSY` or `UNAVAILABLE`.
fn busy_wait(error: &ErrorReply) -> Duration {
    match error.retry_after_ms {
        0 => BUSY_WAIT,
        ms => Duration::from_millis(ms.into()),
    }
}

/// What the phone's refusal `code` means for the user.
fn refused(code: ErrorCode) -> PhotoError {
    match code {
        ErrorCode::NotFound => PhotoError::NotFound,
        ErrorCode::Disabled | ErrorCode::PermissionRequired | ErrorCode::Unsupported => {
            PhotoError::NotReady
        }
        _ => PhotoError::Failed,
    }
}

fn answer_all<T: Clone>(
    waiters: Vec<oneshot::Sender<Result<T, PhotoError>>>,
    result: &Result<T, PhotoError>,
) {
    for waiter in waiters {
        // A receiver that is gone no longer wants it.
        drop(waiter.send(result.clone()));
    }
}

/// Why the photos cannot be followed in `session`, where the feature is
/// not ready on both devices.
fn not_ready<S>(session: &Session<S>) -> PhotosState
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if !session.shared_features().contains(&Feature::Photos) {
        return PhotosState::Unsupported;
    }
    let Some(states) = session.peer_availability() else {
        return PhotosState::Waiting;
    };
    match states.get(&Feature::Photos) {
        Some(Availability::Disabled) => PhotosState::TurnedOff,
        Some(Availability::PermissionRequired(_)) => PhotosState::PermissionRequired,
        _ => PhotosState::Unavailable,
    }
}
