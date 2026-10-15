//! A shell owns the controlling session while the tested program exits.
//! Darwin revokes the terminal when its session leader dies, so post-exit
//! echo/termios assertions must run before the shell itself is torn down.
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

pub(crate) struct TerminalChild {
    process: Child,
    status: Option<PathBuf>,
}

pub(crate) fn command(socket: &Path) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("\"$@\"; code=$?; printf '%s\\n' \"$code\" > \"$PA_PTY_EXIT_STATUS.tmp\"; /bin/mv \"$PA_PTY_EXIT_STATUS.tmp\" \"$PA_PTY_EXIT_STATUS\"; while IFS= read -r line; do :; done")
        .arg("pty-owner")
        .arg(std::env::current_exe().expect("test binary"))
        .env("PA_PTY_EXIT_STATUS", status_path(socket));
    command
}

fn status_path(socket: &Path) -> PathBuf {
    socket.with_extension("product-exit")
}

impl TerminalChild {
    pub(crate) fn shell(process: Child, socket: &Path) -> Self {
        Self {
            process,
            status: Some(status_path(socket)),
        }
    }

    pub(crate) fn direct(process: Child) -> Self {
        Self {
            process,
            status: None,
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.process.id()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let Some(path) = &self.status else {
            return self.process.try_wait();
        };
        match std::fs::read_to_string(path) {
            Ok(text) if !text.is_empty() => {
                let code: i32 = text.trim().parse().map_err(io::Error::other)?;
                Ok(Some(ExitStatus::from_raw(code << 8)))
            }
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if let Some(status) = self.process.try_wait()? {
                    return Err(io::Error::other(format!(
                        "terminal owner exited before reporting product status: {status}"
                    )));
                }
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            // The fixture created this private session/process group.
            let result = unsafe { libc::kill(-(self.process.id() as i32), libc::SIGKILL) };
            if result == 0 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(libc::ESRCH) {
                Ok(())
            } else {
                Err(error)
            };
        }
        self.process.kill()
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.process.wait()
    }
}
