use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};

/// Copies `text` to the system clipboard and returns the mechanism used.
///
/// Local sessions prefer the platform clipboard command. SSH sessions, or local sessions without one, fall back to an
/// OSC 52 escape on an interactive stdout so the user's terminal sets its clipboard. OSC 52 delivery cannot be
/// confirmed, and terminals that disable it ignore the escape.
///
/// # Errors
///
/// Returns an error when a clipboard command fails, or no command exists and stdout is not a terminal.
pub fn copy(text: &str) -> Result<&'static str, String> {
    let remote = std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    let mut candidates: Vec<(&'static str, &'static [&'static str])> = Vec::new();
    if !remote {
        if cfg!(target_os = "macos") {
            candidates.push(("pbcopy", &[]));
        }

        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            candidates.push(("wl-copy", &[]));
        }

        if std::env::var_os("DISPLAY").is_some() {
            candidates.push(("xclip", &["-selection", "clipboard"]));
            candidates.push(("xsel", &["--clipboard", "--input"]));
        }
    }

    for (program, args) in candidates {
        let mut child = match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot start {program}: {error}")),
        };
        let written = child
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(text.as_bytes()));
        let status = child
            .wait()
            .map_err(|error| format!("cannot wait for {program}: {error}"))?;
        written.map_err(|error| format!("cannot write to {program}: {error}"))?;
        if !status.success() {
            return Err(format!("{program} exited with {status}"));
        }

        return Ok(program);
    }

    let mut stdout = io::stdout();
    if !stdout.is_terminal() {
        return Err("no clipboard command found and stdout is not a terminal".to_owned());
    }

    write!(stdout, "\x1b]52;c;{}\x07", base64(text.as_bytes()))
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("cannot write OSC 52 escape: {error}"))?;
    Ok("terminal (OSC 52)")
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let block = chunk.iter().enumerate().fold(0_u32, |block, (index, byte)| {
            block | u32::from(*byte) << (16 - 8 * index)
        });
        for position in 0..4 {
            if position <= chunk.len() {
                encoded.push(char::from(ALPHABET[(block >> (18 - 6 * position) & 0x3f) as usize]));
            } else {
                encoded.push('=');
            }
        }
    }

    encoded
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_rfc_4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(super::base64(input.as_bytes()), expected);
        }
    }
}
