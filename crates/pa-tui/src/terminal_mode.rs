//! Raw-mode entry shared by terminal surfaces and their resume paths.

pub(crate) fn enable_raw_mode() -> std::io::Result<()> {
    // A cooked shell may have received XOFF. Darwin does not resume output
    // merely because cfmakeraw clears IXON. Lift the runtime stop before
    // any terminal writes; crossterm still owns exact termios restoration.
    pa_types::platform::terminal::restart_output();
    crossterm::terminal::enable_raw_mode()
}
