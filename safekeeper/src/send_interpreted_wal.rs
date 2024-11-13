use std::time::Duration;

use anyhow::Context;
use futures::StreamExt;
use pageserver_api::shard::ShardIdentity;
use postgres_backend::{CopyStreamHandlerEnd, PostgresBackend};
use postgres_ffi::{get_current_timestamp, waldecoder::WalStreamDecoder};
use pq_proto::{BeMessage, InterpretedWalRecordsBody, WalSndKeepAlive};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::MissedTickBehavior;
use utils::bin_ser::BeSer;
use utils::lsn::Lsn;
use wal_decoder::models::InterpretedWalRecord;

use crate::send_wal::EndWatchView;
use crate::wal_reader_stream::{WalBytes, WalReaderStreamBuilder};

/// Shard-aware interpreted record sender.
/// This is used for sending WAL to the pageserver. Said WAL
/// is pre-interpreted and filtered for the shard.
pub(crate) struct InterpretedWalSender<'a, IO> {
    pub(crate) pgb: &'a mut PostgresBackend<IO>,
    pub(crate) wal_stream_builder: WalReaderStreamBuilder,
    pub(crate) end_watch_view: EndWatchView,
    pub(crate) shard: ShardIdentity,
    pub(crate) pg_version: u32,
    pub(crate) appname: Option<String>,
}

impl<IO: AsyncRead + AsyncWrite + Unpin> InterpretedWalSender<'_, IO> {
    /// Send interpreted WAL to a receiver.
    /// Stops when an error occurs or the receiver is caught up and there's no active compute.
    ///
    /// Err(CopyStreamHandlerEnd) is always returned; Result is used only for ?
    /// convenience.
    pub(crate) async fn run(self) -> Result<(), CopyStreamHandlerEnd> {
        let mut wal_position = self.wal_stream_builder.start_pos();
        let mut wal_decoder =
            WalStreamDecoder::new(self.wal_stream_builder.start_pos(), self.pg_version);

        let stream = self.wal_stream_builder.build().await?;
        let mut stream = std::pin::pin!(stream);

        let mut keepalive_ticker = tokio::time::interval(Duration::from_secs(1));
        keepalive_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        keepalive_ticker.reset();

        loop {
            tokio::select! {
                // Get some WAL from the stream and then: decode, interpret and send it
                wal = stream.next() => {
                    let WalBytes { wal, wal_start_lsn, wal_end_lsn, available_wal_end_lsn } = match wal {
                        Some(some) => some?,
                        None => { break; }
                    };

                    wal_position = wal_start_lsn;
                    wal_decoder.feed_bytes(&wal);

                    let mut records = Vec::new();
                    let mut max_next_record_lsn = None;
                    while let Some((next_record_lsn, recdata)) = wal_decoder
                        .poll_decode()
                        .with_context(|| "Failed to decode WAL")?
                    {
                        assert!(next_record_lsn.is_aligned());
                        max_next_record_lsn = Some(next_record_lsn);

                        // Deserialize and interpret WAL record
                        let interpreted = InterpretedWalRecord::from_bytes_filtered(
                            recdata,
                            &self.shard,
                            next_record_lsn,
                            self.pg_version,
                        )
                        .with_context(|| "Failed to interpret WAL")?;

                        if !interpreted.is_empty() {
                            records.push(interpreted);
                        }
                    }

                    let mut buf = Vec::new();
                    records
                        .ser_into(&mut buf)
                        .with_context(|| "Failed to serialize interpreted WAL")?;

                    // Reset the keep alive ticker since we are sending something
                    // over the wire now.
                    keepalive_ticker.reset();

                    self.pgb
                        .write_message(&BeMessage::InterpretedWalRecords(InterpretedWalRecordsBody {
                            streaming_lsn: wal_end_lsn.0,
                            safekeeper_wal_end_lsn: available_wal_end_lsn.0,
                            next_record_lsn: max_next_record_lsn.unwrap_or(Lsn::INVALID).0,
                            data: buf.as_slice(),
                        })).await?;
                }

                // Send a periodic keep alive when the connection has been idle for a while.
                _ = keepalive_ticker.tick() => {
                    self.pgb
                        .write_message(&BeMessage::KeepAlive(WalSndKeepAlive {
                            wal_end: self.end_watch_view.get().0,
                            timestamp: get_current_timestamp(),
                            request_reply: true,
                        }))
                        .await?;
                }
            }
        }

        // The loop above ends when the receiver is caught up and there's no more WAL to send.
        Err(CopyStreamHandlerEnd::ServerInitiated(format!(
            "ending streaming to {:?} at {}, receiver is caughtup and there is no computes",
            self.appname, wal_position,
        )))
    }
}
