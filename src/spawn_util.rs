//! Shared helper for spawning child processes without flashing a console
//! window on Windows.

/// Apply `CREATE_NO_WINDOW` so a spawned console program (cargo, rustup,
/// rust-analyzer, …) does NOT briefly pop a console window that steals focus
/// from the GUI — which made the app window flicker and the taskbar flash as
/// if many instances were launching. No-op on non-Windows.
///
/// Takes the `Command` by value and returns it so it drops straight into a
/// builder chain: `no_window(Command::new("cargo")).args(..).spawn()`.
pub fn no_window(cmd: std::process::Command) -> std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        return cmd;
    }
    #[cfg(not(windows))]
    cmd
}
