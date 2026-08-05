use nix::poll::{self, PollFd, PollFlags, PollTimeout};
use std::fmt;
use std::io::{self, BufWriter, Write};
use std::os::unix::io::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::process::{Child, ChildStdin, Stdio};

pub const CANDIDATE_PROCESS_NAME: &str = "kime-candidate-window";

/// Time we are willing to block the input thread waiting for the child to answer.
const READY_TIMEOUT_MS: u16 = 200;

/// A handle to a running `kime-candidate-window`.
///
/// The child is a pure renderer: it never reads the keyboard. The input method
/// owns the keyboard and drives the display over stdin. On Wayland this is not
/// merely a preference -- an input method holds an exclusive, seat wide
/// `zwp_input_method_keyboard_grab_v2`, so the child cannot receive key events
/// even when it has focus.
///
/// # Wire format
///
/// Parent to child, one command per line. stdin stays open for the whole
/// session:
///
/// * `c\t<key>\t<value>` -- one candidate, repeated
/// * `end` -- end of the candidate list
/// * `page <n>` -- show page `n`, zero based
/// * `hl <n>` -- highlight absolute candidate index `n`
/// * `quit` -- exit without selecting
///
/// Child to parent: the absolute index of a mouse clicked candidate, as a
/// decimal integer on its own line, then exit. Replying with an index rather
/// than the candidate text keeps the reply immune to escaping problems and lets
/// the engine resolve it against the dictionary slice it already owns.
pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Client {
    pub fn new(candidate_list: &[(&str, &str)]) -> io::Result<Self> {
        let mut child = std::process::Command::new(CANDIDATE_PROCESS_NAME)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut stdin = child.stdin.take().expect("stdin is piped");

        {
            let mut writer = BufWriter::new(&mut stdin);

            for (key, value) in candidate_list {
                writeln!(writer, "c\t{}\t{}", sanitize(key), sanitize(value))?;
            }

            writeln!(writer, "end")?;
            writer.flush()?;
        }

        Ok(Self {
            child,
            stdin: Some(stdin),
        })
    }

    /// Raw fd carrying the child's reply, for registration in a frontend event
    /// loop. Readable means the session ended: either the user clicked a
    /// candidate, or the child died.
    pub fn ready_fd(&self) -> RawFd {
        self.child
            .stdout
            .as_ref()
            .expect("stdout is piped")
            .as_raw_fd()
    }

    fn poll_stdout(&self, timeout: PollTimeout) -> bool {
        let stdout: BorrowedFd = self.child.stdout.as_ref().expect("stdout is piped").as_fd();
        let mut fds = [PollFd::new(stdout, PollFlags::POLLIN)];

        match poll::poll(&mut fds, timeout) {
            Ok(n) if n > 0 => fds[0].revents().is_some_and(|revents| {
                // POLLHUP without POLLIN means the child died. That is still a
                // reason to wake up and reap it; `close` is what tells a real
                // selection apart from a corpse.
                revents.intersects(PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR)
            }),
            _ => false,
        }
    }

    /// Has the session ended? Blocks for up to [`READY_TIMEOUT_MS`].
    pub fn is_ready(&self) -> bool {
        self.poll_stdout(PollTimeout::from(READY_TIMEOUT_MS))
    }

    /// Non-blocking variant, for callers driven by an event loop.
    pub fn is_ready_now(&self) -> bool {
        self.poll_stdout(PollTimeout::ZERO)
    }

    fn send(&mut self, command: &str) -> io::Result<()> {
        match self.stdin.as_mut() {
            Some(stdin) => {
                writeln!(stdin, "{command}")?;
                stdin.flush()
            }
            None => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "candidate window stdin is closed",
            )),
        }
    }

    pub fn set_page(&mut self, page: usize) -> io::Result<()> {
        self.send(&format!("page {page}"))
    }

    pub fn set_highlight(&mut self, index: usize) -> io::Result<()> {
        self.send(&format!("hl {index}"))
    }

    /// Collect the selected index. Only call once [`Client::is_ready`] reports
    /// true; otherwise it blocks until the child exits.
    ///
    /// `Ok(None)` means the user dismissed the window. `Err` means the child
    /// died or spoke nonsense -- cases that used to be indistinguishable from
    /// an empty selection, and so silently committed an empty string.
    pub fn close(mut self) -> io::Result<Option<usize>> {
        // Closing stdin lets a well behaved child exit on its own.
        drop(self.stdin.take());

        let output = self.child.wait_with_output()?;

        if !output.status.success() {
            return Err(io::Error::other(format!(
                "{CANDIDATE_PROCESS_NAME} exited with {}",
                output.status
            )));
        }

        let reply = String::from_utf8_lossy(&output.stdout);
        let reply = reply.trim();

        if reply.is_empty() {
            return Ok(None);
        }

        reply.parse::<usize>().map(Some).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{CANDIDATE_PROCESS_NAME} replied with {reply:?}, expected an index"),
            )
        })
    }

    /// Tear the window down without collecting a selection.
    pub fn cancel(mut self) {
        let _ = self.send("quit");
        drop(self.stdin.take());
        // The child exits on `quit`, but never block the input thread on it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Tabs and newlines would desync the framing. Dictionary entries never contain
/// them, but a desynced stream is far harder to diagnose than a mangled
/// description.
fn sanitize(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CandidateClient")
    }
}
