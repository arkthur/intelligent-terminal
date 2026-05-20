// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// Pure functions extracted from shell_manager for fuzzing.
// This module is shared between the library target (for cargo-fuzz)
// and the binary target (via #[path] include in shell_manager.rs).

/// Build a commandline string from a command and its arguments for WT pane
/// creation. This is the string passed to `create_tab`'s `commandline` param.
///
/// # Security note
///
/// This function is a fuzz target — the quoting must be robust against
/// agent-supplied strings containing shell metacharacters.
pub fn build_wt_commandline(command: &str, args: &[String]) -> String {
    // Quote the command if it contains spaces or double quotes
    let mut cmdline = if command.contains(' ') || command.contains('"') {
        let mut quoted = String::with_capacity(command.len() + 2);
        quoted.push('"');
        for ch in command.chars() {
            if ch == '"' {
                quoted.push('"');
            }
            quoted.push(ch);
        }
        quoted.push('"');
        quoted
    } else {
        command.to_string()
    };

    for arg in args {
        cmdline.push(' ');
        // Always quote empty args, and args containing spaces or double quotes
        if arg.is_empty() || arg.contains(' ') || arg.contains('"') {
            cmdline.push('"');
            // Escape embedded double quotes by doubling them
            for ch in arg.chars() {
                if ch == '"' {
                    cmdline.push('"');
                }
                cmdline.push(ch);
            }
            cmdline.push('"');
        } else {
            cmdline.push_str(arg);
        }
    }
    cmdline
}
