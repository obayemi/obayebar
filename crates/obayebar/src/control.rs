//! The bar's end of the control socket.
//!
//! `obayebar-launcher` writes one line here and exits; the bar, which already
//! holds the parsed entry list and the decoded icons, puts the launcher on
//! screen. The socket mechanics and the command vocabulary live in
//! `obayebar_core::control` so that client can speak them without linking iced.

use std::time::Duration;

use futures_util::Stream;
use obayebar_core::control::{self, BarCommand, BAR_SOCKET};
use tokio::io::AsyncBufReadExt as _;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// How long a connected client has to send its line before it is dropped.
///
/// The accept loop handles one connection at a time, so a client that connects
/// and says nothing would otherwise wedge the launcher keybinding for good.
const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Commands sent by `obayebar-launcher` (or anything else writing to the
/// socket).
///
/// A bar that cannot bind the socket keeps running: everything else it draws
/// still works, and saying so once is more useful than refusing to start.
pub fn stream() -> impl Stream<Item = BarCommand> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();

    tokio::spawn(async move {
        let listener = match control::bind(BAR_SOCKET) {
            Ok(listener) => listener,
            Err(err) => {
                log::warn!("control: no command socket ({err})");
                return;
            }
        };
        let listener = match tokio::net::UnixListener::from_std(listener) {
            Ok(listener) => listener,
            Err(err) => {
                log::warn!("control: cannot watch the command socket ({err})");
                return;
            }
        };
        log::info!("control: listening on {BAR_SOCKET}");

        loop {
            let stream = match listener.accept().await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    log::warn!("control: accept failed ({err})");
                    continue;
                }
            };

            let mut line = String::new();
            let read = tokio::time::timeout(
                READ_TIMEOUT,
                tokio::io::BufReader::new(stream).read_line(&mut line),
            )
            .await;
            match read {
                Ok(Ok(_)) => match BarCommand::parse(&line) {
                    Some(command) => {
                        if tx.send(command).is_err() {
                            return;
                        }
                    }
                    None => log::warn!("control: ignoring command {:?}", line.trim()),
                },
                Ok(Err(err)) => log::warn!("control: unreadable command ({err})"),
                Err(_) => log::warn!("control: client sent nothing within {READ_TIMEOUT:?}"),
            }
        }
    });

    UnboundedReceiverStream::new(rx)
}
