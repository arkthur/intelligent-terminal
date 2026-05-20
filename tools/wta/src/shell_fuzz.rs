// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// Pure functions extracted from shell_manager for fuzzing.
// This module is shared between the library target (for cargo-fuzz)
// and the binary target (via #[path] include in shell_manager.rs).

/// Append a single value (command or argument) to a commandline string,
/// quoting it using the `CommandLineToArgvW` convention when necessary.
fn append_wt_commandline_arg(cmdline: &mut String, value: &str) {
    let needs_quotes = value.is_empty() || value.chars().any(|ch| ch == ' ' || ch == '\t' || ch == '"');
    if !needs_quotes {
        cmdline.push_str(value);
        return;
    }
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
/// creation. This is the string passed to `create_tab`'s `commandline` param.
///
/// # Security note
///
/// This function is a fuzz target — the quoting must be robust against
/// agent-supplied strings containing shell metacharacters.
pub fn build_wt_commandline(command: &str, args: &[String]) -> String {
    let mut cmdline = String::new();
    append_wt_commandline_arg(&mut cmdline, command);
    for arg in args {
        cmdline.push(' ');
        append_wt_commandline_arg(&mut cmdline, arg);
    }
    cmdline
}
