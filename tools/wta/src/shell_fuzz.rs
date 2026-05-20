// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// Pure functions extracted from shell_manager for fuzzing.
// Compiled into the wta library target; the binary and the cargo-fuzz
// target both consume them via `wta::build_wt_commandline`.

/// Error returned by [`build_wt_commandline`] when the input cannot be
/// encoded as a valid Windows commandline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildCommandlineError {
    /// The program path (argv[0]) contains a literal `"`. There is no
    /// `CommandLineToArgvW`-compatible way to escape it.
    QuoteInProgram,
}

impl std::fmt::Display for BuildCommandlineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::QuoteInProgram => {
                f.write_str("executable path cannot contain a literal double quote")
            }
        }
    }
}

impl std::error::Error for BuildCommandlineError {}

/// Quote the program path (argv[0]). `CommandLineToArgvW` uses different
/// rules for the first token: backslashes are literal, and the first
/// unescaped `"` ends argv[0] — there is no way to escape a `"` inside
/// it. So we wrap in plain double quotes and reject inputs containing `"`.
/// (Real executable paths never do; agent-supplied input might.)
fn append_wt_commandline_program(
    cmdline: &mut String,
    value: &str,
) -> Result<(), BuildCommandlineError> {
    if value.contains('"') {
        return Err(BuildCommandlineError::QuoteInProgram);
    }
    cmdline.push('"');
    cmdline.push_str(value);
    cmdline.push('"');
    Ok(())
}

/// Append a non-first argument, quoting using the `CommandLineToArgvW`
/// convention. Always quotes unconditionally — mirrors
/// `QuoteAndEscapeCommandlineArg` in `src/cascadia/WinRTUtils/inc/WtExeUtils.h`.
/// A `needs_quotes` heuristic is fragile because the OS parser splits on
/// whitespace beyond space/tab (e.g. `\n`, `\r`).
fn append_wt_commandline_arg(cmdline: &mut String, value: &str) {
    cmdline.push('"');
    let mut backslashes = 0;
    for ch in value.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                for _ in 0..(backslashes * 2 + 1) {
                    cmdline.push('\\');
                }
                cmdline.push('"');
                backslashes = 0;
            }
            _ => {
                for _ in 0..backslashes {
                    cmdline.push('\\');
                }
                backslashes = 0;
                cmdline.push(ch);
            }
        }
    }
    for _ in 0..(backslashes * 2) {
        cmdline.push('\\');
    }
    cmdline.push('"');
}

/// Build a commandline string from a command and its arguments for WT pane
/// creation. This is the string passed to `create_tab`'s `commandline` param,
/// which WT parses with `CommandLineToArgvW` before handing off to
/// `CreateProcess` — there is no shell in this pipeline, so metacharacters
/// like `&` / `|` / `$` are not special.
///
/// # Security note
///
/// The threat model here is **argument injection**: an agent-supplied
/// substring must not be able to escape its argument boundary and inject
/// additional argv entries. Robustness against the `CommandLineToArgvW`
/// quoting rules (whitespace, `"`, runs of `\`) is what this function —
/// and its fuzz target — has to get right.
pub fn build_wt_commandline(
    command: &str,
    args: &[String],
) -> Result<String, BuildCommandlineError> {
    let mut cmdline = String::new();
    append_wt_commandline_program(&mut cmdline, command)?;
    for arg in args {
        cmdline.push(' ');
        append_wt_commandline_arg(&mut cmdline, arg);
    }
    Ok(cmdline)
}
