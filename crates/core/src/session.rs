//! A session between two paired devices, on top of an established stream.
//!
//! The stream is normally the TLS connection set up by the transport layer,
//! but any `AsyncRead + AsyncWrite` works, which is how the tests run
//! sessions over in-memory pipes.
//!
//! A session starts with the `Hello` exchange: both sides send `Hello` as
//! their first frame, then read the peer's. Each side refuses a peer that
//! speaks another protocol major version or sends anything else first.

use std::collections::BTreeSet;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use link2linux_protocol::{
    DEFAULT_MAX_FRAME_LEN, Envelope, EnvelopeCodec, FrameError, Hello, PROTOCOL_VERSION, Payload,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;
use tracing::debug;

use crate::device::{DeviceInfo, Feature};

/// Tunables for a [`Session`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    /// How long to wait for the peer's `Hello`.
    pub handshake_timeout: Duration,
    /// Largest frame body accepted or sent, in bytes.
    pub max_frame_len: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            handshake_timeout: Duration::from_secs(10),
            max_frame_len: DEFAULT_MAX_FRAME_LEN,
        }
    }
}

/// Why a session failed or ended abnormally.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SessionError {
    /// Reading or writing a frame failed, including I/O errors.
    #[error(transparent)]
    Frame(#[from] FrameError),
    /// The peer's `Hello` did not arrive in time.
    #[error("no Hello from the peer within {0:?}")]
    HandshakeTimeout(Duration),
    /// The peer closed the stream before sending its `Hello`.
    #[error("the peer closed the connection during the handshake")]
    ClosedDuringHandshake,
    /// The peer's first frame was not a `Hello`.
    #[error("the peer's first message was not a Hello")]
    ExpectedHello,
    /// The peer speaks another protocol major version.
    #[error("the peer speaks protocol version {remote}, this device speaks {local}")]
    VersionMismatch {
        /// This device's [`PROTOCOL_VERSION`].
        local: u32,
        /// The version in the peer's `Hello`.
        remote: u32,
    },
    /// The peer's `Hello` is missing required information.
    #[error("invalid Hello from the peer: {0}")]
    InvalidHello(&'static str),
    /// The peer's sequence numbers did not increase.
    #[error("the peer sent sequence number {received} after {previous}")]
    OutOfOrder {
        /// The last sequence number accepted from the peer (0 before any).
        previous: u64,
        /// The offending sequence number.
        received: u64,
    },
}

/// An established session with a peer device.
///
/// Created by [`Session::handshake`].
#[derive(Debug)]
pub struct Session<S> {
    framed: Framed<S, EnvelopeCodec>,
    local: DeviceInfo,
    peer: DeviceInfo,
}

impl<S> Session<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Runs the `Hello` exchange on `io` and returns the established
    /// session.
    ///
    /// This side's `Hello` goes out first, without waiting for the peer, so
    /// both ends may call this at the same time. That relies on the stream
    /// buffering one small `Hello` frame, which every real transport does.
    ///
    /// # Errors
    ///
    /// Fails if the peer's `Hello` does not arrive within
    /// [`SessionConfig::handshake_timeout`], if the peer sends anything else
    /// first, speaks another protocol version, or sends an invalid `Hello`,
    /// and on any framing or I/O error.
    pub async fn handshake(
        io: S,
        local: DeviceInfo,
        config: SessionConfig,
    ) -> Result<Self, SessionError> {
        let codec = EnvelopeCodec::with_max_frame_len(config.max_frame_len);
        let mut framed = Framed::new(io, codec);
        let hello = Envelope {
            seq: 1,
            payload: Some(Payload::Hello(hello_from(&local))),
        };

        let exchange = async {
            framed.send(&hello).await?;
            match framed.next().await {
                Some(frame) => Ok(frame?),
                None => Err(SessionError::ClosedDuringHandshake),
            }
        };
        let first = tokio::time::timeout(config.handshake_timeout, exchange)
            .await
            .map_err(|_| SessionError::HandshakeTimeout(config.handshake_timeout))??;

        check_seq(0, first.seq)?;
        let Some(Payload::Hello(hello)) = first.payload else {
            return Err(SessionError::ExpectedHello);
        };
        let peer = peer_from(hello)?;
        debug!(peer.id, peer.name, ?peer.kind, "session established");

        Ok(Self {
            framed,
            local,
            peer,
        })
    }

    /// This device, as announced to the peer.
    #[must_use]
    pub fn local(&self) -> &DeviceInfo {
        &self.local
    }

    /// The peer device, as it announced itself.
    #[must_use]
    pub fn peer(&self) -> &DeviceInfo {
        &self.peer
    }

    /// Features both devices announced, and so can use in this session.
    #[must_use]
    pub fn shared_features(&self) -> BTreeSet<Feature> {
        self.local
            .features
            .intersection(&self.peer.features)
            .copied()
            .collect()
    }

    /// Flushes pending frames and shuts the stream down.
    ///
    /// # Errors
    ///
    /// Fails if the final flush or the shutdown fails.
    pub async fn close(mut self) -> Result<(), SessionError> {
        self.framed.close().await?;
        Ok(())
    }
}

/// Builds this device's `Hello`.
fn hello_from(local: &DeviceInfo) -> Hello {
    Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: local.id.clone(),
        device_name: local.name.clone(),
        kind: local.kind.into(),
        features: local
            .features
            .iter()
            .map(|feature| feature.name().to_owned())
            .collect(),
    }
}

/// Validates the peer's `Hello` and turns it into a [`DeviceInfo`].
fn peer_from(hello: Hello) -> Result<DeviceInfo, SessionError> {
    if hello.protocol_version != PROTOCOL_VERSION {
        return Err(SessionError::VersionMismatch {
            local: PROTOCOL_VERSION,
            remote: hello.protocol_version,
        });
    }
    if hello.device_id.is_empty() {
        return Err(SessionError::InvalidHello("empty device_id"));
    }
    // Device kinds and features added by newer versions read as unknown.
    let kind = hello.kind();
    let mut features = BTreeSet::new();
    for name in &hello.features {
        match Feature::from_name(name) {
            Some(feature) => {
                features.insert(feature);
            }
            None => debug!(feature = %name, "ignoring a feature this build does not know"),
        }
    }
    Ok(DeviceInfo {
        id: hello.device_id,
        name: hello.device_name,
        kind,
        features,
    })
}

/// Checks that a sequence number from the peer is above the last one.
fn check_seq(previous: u64, received: u64) -> Result<(), SessionError> {
    if received > previous {
        Ok(())
    } else {
        Err(SessionError::OutOfOrder { previous, received })
    }
}
