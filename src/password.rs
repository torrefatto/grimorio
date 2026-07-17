#![allow(dead_code)]

/// Trait for reading a secret from the user.
///
/// Implementations can use terminal input, OS credential prompts, or graphical
/// dialogs. The trait is object-safe so the backend can be swapped at runtime.
pub trait PasswordReader: Send + Sync {
    /// Read a secret string from the user, displaying `prompt` if possible.
    fn read_password(&self, prompt: &str) -> Result<String, std::io::Error>;
}

// ---------------------------------------------------------------------------
// Terminal reader (default)
// ---------------------------------------------------------------------------

/// Reads a password from the terminal without echoing.
pub struct TerminalPasswordReader;

impl PasswordReader for TerminalPasswordReader {
    fn read_password(&self, prompt: &str) -> Result<String, std::io::Error> {
        use std::io::{IsTerminal, Write};

        eprint!("{}", prompt);
        std::io::stderr().flush()?;

        // When stdin is not a terminal (a pipe or redirect, as in scripts and
        // tests), read the secret as a plain line from stdin. No-echo terminal
        // reads only make sense for an interactive TTY.
        if !std::io::stdin().is_terminal() {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            return Ok(line.trim_end_matches(|c| c == '\r' || c == '\n').to_string());
        }

        #[cfg(unix)]
        {
            rpassword::read_password()
        }
        #[cfg(windows)]
        {
            windows_sys::Win32::System::Console::read_password_win(prompt)
        }
    }
}

// ---------------------------------------------------------------------------
// Windows console implementation (no echo)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows_sys {
    pub use windows_sys::Win32::System::Console;

    pub trait ReadPasswordWin {
        fn read_password_win(prompt: &str) -> Result<String, std::io::Error>;
    }

    impl ReadPasswordWin for Console {
        fn read_password_win(prompt: &str) -> Result<String, std::io::Error> {
            use std::io::Write;
            use windows_sys::Win32::System::Console::{
                GetStdHandle, SetConsoleMode, CONSOLE_MODE, ENABLE_ECHO_INPUT,
                ENABLE_LINE_INPUT, STD_INPUT_HANDLE,
            };

            unsafe {
                let stdin = GetStdHandle(STD_INPUT_HANDLE);
                let mut original_mode: CONSOLE_MODE = 0;
                windows_sys::Win32::System::Console::GetConsoleMode(
                    stdin,
                    &mut original_mode,
                );

                // Disable echo
                let no_echo = (original_mode & !ENABLE_ECHO_INPUT) | ENABLE_LINE_INPUT;
                SetConsoleMode(stdin, no_echo);

                eprint!("{}", prompt);
                std::io::stdout().flush()?;

                let mut buffer = String::new();
                std::io::stdin().read_line(&mut buffer)?;

                // Restore original mode
                SetConsoleMode(stdin, original_mode);

                Ok(buffer.trim_end_matches(|c| c == '\r' || c == '\n').to_string())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stub reader (for testing / headless environments)
// ---------------------------------------------------------------------------

/// A reader that always returns an error. Useful for tests or CI.
pub struct FailingReader;

impl PasswordReader for FailingReader {
    fn read_password(&self, _prompt: &str) -> Result<String, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "no password reader available",
        ))
    }
}
