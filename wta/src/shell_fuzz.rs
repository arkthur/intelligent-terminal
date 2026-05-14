// Copyright (c) Microsoft Corporation.
// Licensed under the MIT license.
//
// Pure functions extracted from shell_manager for fuzzing.
// This module is compiled into the library target only and has
// no dependencies on the binary-specific module tree.

/// Build a commandline string from a command and its arguments for WT pane
/// creation. This is the string passed to `create_tab`'s `commandline` param.
///
/// # Security note
///
/// This function is a fuzz target — the quoting must be robust against
/// agent-supplied strings containing shell metacharacters.
pub fn build_wt_commandline(command: &str, args: &[String]) -> String {
    let mut cmdline = command.to_string();
    for arg in args {
        cmdline.push(' ');
        // Quote args containing spaces or double quotes
        if arg.contains(' ') || arg.contains('"') {
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
