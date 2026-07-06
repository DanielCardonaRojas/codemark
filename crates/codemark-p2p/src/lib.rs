//! Optional peer-to-peer transport for Codemark.
//!
//! This crate is deliberately **tour-agnostic**: it moves opaque byte buffers
//! between two machines over an [iroh] QUIC connection, addressed by a
//! shareable [`Ticket`]. It knows nothing about tours, packs, serialization,
//! encryption, or the database — the CLI builds the pack bytes (via
//! `codemark-core`), optionally encrypts them, and hands them here. Keeping the
//! boundary at raw bytes is what confines iroh's dependency tree (and the
//! QUIC/relay stack) to this single, optional crate.
//!
//! The transport uses [`iroh-blobs`]: bytes are stored as a BLAKE3
//! content-addressed blob, and the receiver verifies the hash on download, so a
//! transfer is either byte-for-byte correct or it fails.
//!
//! # Model
//! - [`push_bytes`] adds the bytes to an in-memory blob store and returns a
//!   [`Ticket`] plus a [`Provider`] guard. **The provider must stay alive**
//!   (the process must keep running) until the peer has pulled — iroh relays
//!   forward live packets, they do not store data.
//! - [`pull_bytes`] dials the peer named in the ticket, downloads the blob,
//!   verifies it, and returns the bytes.
//!
//! [iroh]: https://iroh.computer
//! [`iroh-blobs`]: https://docs.rs/iroh-blobs

use std::time::Duration;

use anyhow::{Context, Result};
use iroh::{Endpoint, EndpointAddr, endpoint::presets, protocol::Router};
use iroh_blobs::provider::events::{
    ConnectMode, EventMask, EventSender, ProviderMessage, RequestMode,
};
use iroh_blobs::{BlobsProtocol, api::TempTag, store::mem::MemStore, ticket::BlobTicket};

/// A self-contained, shareable string (`blob…`) that names a blob and the peer
/// to fetch it from. Pass it out-of-band (Slack, 1Password, …) to a receiver.
pub type Ticket = String;

/// How long to wait for the local endpoint to acquire a dialable address
/// (direct socket addresses and/or a home relay) before giving up.
const ADDR_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait to reach the sender before reporting it as unreachable. Sits
/// just above iroh's internal ~10s connect timeout so its error usually surfaces
/// first; this is a backstop for cases (e.g. a stalled relay path) where connect
/// would otherwise hang longer.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Keeps a pushed blob available to peers.
///
/// Dropping (or [`shutdown`](Provider::shutdown)-ing) this guard tears down the
/// node and stops serving the blob. Hold it for as long as you want the tour to
/// be pullable.
pub struct Provider {
    router: Router,
    // Held only to keep the backing store and the blob's tag alive for the
    // lifetime of the provider; the router serves reads from this store.
    _store: MemStore,
    _tag: TempTag,
    // Fires each time a peer finishes downloading the blob (see `recv_delivery`).
    delivery_rx: tokio::sync::mpsc::Receiver<()>,
}

impl Provider {
    /// Resolves once a peer has downloaded the tour — detected as a get request
    /// followed by that connection closing. Callers can use this to report
    /// "delivered" and/or stop serving. May resolve more than once if the tour is
    /// pulled repeatedly; returns `None` when serving has ended.
    pub async fn recv_delivery(&mut self) -> Option<()> {
        self.delivery_rx.recv().await
    }

    /// Gracefully shut down the provider node.
    pub async fn shutdown(self) -> Result<()> {
        self.router.shutdown().await.context("failed to shut down iroh router")
    }
}

/// Publish `bytes` and return a shareable [`Ticket`] plus the [`Provider`] guard
/// that keeps them available.
///
/// The returned provider must be kept alive until the receiver has finished
/// [`pull_bytes`]; drop it to stop serving.
pub async fn push_bytes(bytes: Vec<u8>) -> Result<(Ticket, Provider)> {
    // `presets::N0` selects the ring crypto backend and enables the n0 relays +
    // DNS address lookup, giving NAT traversal with relay fallback out of the box.
    let endpoint = Endpoint::bind(presets::N0).await.context("failed to bind iroh endpoint")?;
    push_bytes_on(endpoint, bytes).await
}

/// Download the blob named by `ticket` and return its bytes.
///
/// The transfer is BLAKE3-verified: the returned bytes are guaranteed to match
/// the hash embedded in the ticket, or this returns an error.
pub async fn pull_bytes(ticket: &str) -> Result<Vec<u8>> {
    let endpoint = Endpoint::bind(presets::N0).await.context("failed to bind iroh endpoint")?;
    let result = pull_bytes_on(&endpoint, ticket, CONNECT_TIMEOUT).await;
    endpoint.close().await;
    result
}

/// Core of [`push_bytes`], parameterized over the endpoint so tests can supply a
/// relay-free, discovery-free [`presets::Minimal`] endpoint for fully offline
/// round-trips.
async fn push_bytes_on(endpoint: Endpoint, bytes: Vec<u8>) -> Result<(Ticket, Provider)> {
    let store = MemStore::new();

    // Add the bytes as a content-addressed blob. The returned temp tag pins the
    // blob against garbage collection while the provider is alive.
    let tag =
        store.add_bytes(bytes).temp_tag().await.context("failed to add bytes to blob store")?;

    // A ticket is only useful if it tells the receiver how to reach us, so wait
    // until the endpoint has a dialable address before minting one.
    let addr = wait_for_addr(&endpoint).await?;
    let ticket = BlobTicket::new(addr, tag.hash(), tag.format());

    // Observe provider events to detect delivery. `Notify` modes are
    // fire-and-forget (unlike `Intercept`), so this can never gate or stall a
    // transfer. A get request followed by that connection closing means a peer
    // finished pulling — safe to report and to stop serving on.
    let mask = EventMask {
        connected: ConnectMode::Notify,
        get: RequestMode::Notify,
        ..EventMask::DEFAULT
    };
    let (events, mut event_rx) = EventSender::channel(32, mask);
    let (delivery_tx, delivery_rx) = tokio::sync::mpsc::channel::<()>(4);
    tokio::spawn(async move {
        let mut saw_get = false;
        while let Some(msg) = event_rx.recv().await {
            match msg {
                ProviderMessage::GetRequestReceivedNotify(_) => saw_get = true,
                ProviderMessage::ConnectionClosed(_) if saw_get => {
                    saw_get = false;
                    if delivery_tx.send(()).await.is_err() {
                        break; // provider dropped; stop observing
                    }
                }
                _ => {}
            }
        }
    });

    // Route incoming blobs requests to the store.
    let blobs = BlobsProtocol::new(&store, Some(events));
    let router = Router::builder(endpoint).accept(iroh_blobs::ALPN, blobs).spawn();

    Ok((ticket.to_string(), Provider { router, _store: store, _tag: tag, delivery_rx }))
}

/// Core of [`pull_bytes`], parameterized over the endpoint and connect timeout
/// for offline tests.
async fn pull_bytes_on(
    endpoint: &Endpoint,
    ticket: &str,
    connect_timeout: Duration,
) -> Result<Vec<u8>> {
    let trimmed = ticket.trim();
    let ticket: BlobTicket = trimmed.parse().map_err(|e| {
        // A `blob…` string of the wrong base32 length means a character was
        // dropped/added — almost always terminal copy truncation. Say so, since
        // trimming can't recover a character lost mid-string.
        anyhow::anyhow!(
            "invalid ticket: {e} (received {} chars). If you copied it from a terminal it was \
             likely truncated — re-copy the whole `blob…` string (widen the window so it isn't \
             wrapped).",
            trimmed.len(),
        )
    })?;

    // Dial the provider directly using the full address in the ticket (relay +
    // direct paths). A p2p pull needs the sender online at the same time, so an
    // unreachable node is the common failure — bound the wait ourselves (iroh's
    // internal ~10s timeout is a backstop) and explain the likely cause rather
    // than surfacing a bare connection error.
    let offline_hint = "make sure the sender is still running \
        `codemark tour push --p2p` and is online — a peer-to-peer pull needs both \
        machines online at the same time";
    let connection = match tokio::time::timeout(
        connect_timeout,
        endpoint.connect(ticket.addr().clone(), iroh_blobs::ALPN),
    )
    .await
    {
        Ok(Ok(connection)) => connection,
        Ok(Err(e)) => {
            anyhow::bail!("couldn't reach the sender ({e}) — {offline_hint}");
        }
        Err(_) => {
            anyhow::bail!(
                "timed out after {}s trying to reach the sender — {offline_hint}",
                connect_timeout.as_secs(),
            );
        }
    };

    let (bytes, _stats) = iroh_blobs::get::request::get_blob(connection, ticket.hash())
        .bytes_and_stats()
        .await
        .context("failed to download the tour from the sender")?;

    Ok(bytes.to_vec())
}

/// Poll the endpoint until it reports a dialable address (direct and/or relay),
/// so the minted ticket actually points somewhere. Errors on timeout.
async fn wait_for_addr(endpoint: &Endpoint) -> Result<EndpointAddr> {
    let deadline = tokio::time::Instant::now() + ADDR_TIMEOUT;
    loop {
        let addr = endpoint.addr();
        if !addr.is_empty() {
            return Ok(addr);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("endpoint acquired no dialable address within {ADDR_TIMEOUT:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fully offline round-trip: two relay-free `Minimal` endpoints on this host
    /// exchange a blob over their direct addresses. Proves the byte transport
    /// works without any network or n0 infrastructure.
    #[tokio::test]
    async fn round_trip_over_direct_addresses() -> Result<()> {
        let payload = b"codemark tour pack bytes \x00\x01\x02 \xff".to_vec();

        let provider_ep = Endpoint::bind(presets::Minimal).await?;
        let (ticket, mut provider) = push_bytes_on(provider_ep, payload.clone()).await?;

        let receiver_ep = Endpoint::bind(presets::Minimal).await?;
        let received = pull_bytes_on(&receiver_ep, &ticket, CONNECT_TIMEOUT).await?;
        receiver_ep.close().await;

        assert_eq!(received, payload);

        // The provider should observe the completed pull as a delivery.
        let delivered =
            tokio::time::timeout(Duration::from_secs(5), provider.recv_delivery()).await;
        assert!(matches!(delivered, Ok(Some(()))), "expected a delivery signal, got {delivered:?}");

        provider.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_malformed_ticket() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let err = pull_bytes_on(&endpoint, "not-a-ticket", CONNECT_TIMEOUT).await.unwrap_err();
        assert!(err.to_string().contains("invalid ticket"), "got: {err}");
        endpoint.close().await;
    }

    /// Pulling from a node that is offline (its endpoint was bound to mint a
    /// valid ticket, then closed) reports the sender as unreachable rather than
    /// hanging or surfacing a bare connection error.
    #[tokio::test]
    async fn reports_offline_sender() {
        use iroh_blobs::{BlobFormat, Hash};

        // Bind an endpoint to obtain a valid, dialable address, mint a ticket for
        // it, then close it so nothing is listening.
        let dead = Endpoint::bind(presets::Minimal).await.unwrap();
        let addr = wait_for_addr(&dead).await.unwrap();
        let ticket = BlobTicket::new(addr, Hash::new(b"absent-tour"), BlobFormat::Raw);
        dead.close().await;

        let puller = Endpoint::bind(presets::Minimal).await.unwrap();
        let err =
            pull_bytes_on(&puller, &ticket.to_string(), Duration::from_secs(1)).await.unwrap_err();
        assert!(err.to_string().contains("reach the sender"), "got: {err}");
        puller.close().await;
    }
}
