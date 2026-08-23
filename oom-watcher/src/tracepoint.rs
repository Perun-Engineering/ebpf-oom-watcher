//! Startup validation of the `oom:mark_victim` trace-entry layout.
//!
//! The probe decodes the trace entry as a fixed `#[repr(C)]` struct, so it is only correct
//! for the layout the kernel actually emits. That layout changed in Linux 6.9: before it,
//! `mark_victim` carried nothing but `pid`.
//!
//! The dangerous part is that the mismatch is silent. `attach("oom", "mark_victim")` still
//! succeeds on an older kernel — the tracepoint exists, it is just smaller — and the probe
//! then reads 72 bytes off a 12-byte entry. `pid` (offset 8) stays correct while every
//! memory figure becomes adjacent trace-buffer noise, which reads like a units bug rather
//! than a format mismatch.
//!
//! So the layout is asserted against the live `format` file before the program is loaded,
//! and a mismatch is a hard startup failure. Parsing is kept separate from I/O so the whole
//! decision is unit-testable against captured `format` files.
#![cfg_attr(not(feature = "ebpf"), allow(dead_code))]

use std::{fmt, fs, io, path::Path};

use anyhow::{anyhow, Result};

/// The two mount points a `format` file can live behind: tracefs proper, and the older
/// debugfs location. Checked in order.
const FORMAT_PATHS: [&str; 2] = [
    "/sys/kernel/tracing/events/oom/mark_victim/format",
    "/sys/kernel/debug/tracing/events/oom/mark_victim/format",
];

/// The layout `MarkVictimArgs` in the eBPF probe hard-codes: `(name, offset, size)` for
/// every field it reads. Matches Linux 6.9 through at least 6.19; `comm` is a `__data_loc`,
/// hence 4 bytes here and the string itself elsewhere in the entry.
const REQUIRED_FIELDS: [Field; 9] = [
    Field::new("pid", 8, 4),
    Field::new("comm", 12, 4),
    Field::new("total_vm", 16, 8),
    Field::new("anon_rss", 24, 8),
    Field::new("file_rss", 32, 8),
    Field::new("shmem_rss", 40, 8),
    Field::new("uid", 48, 4),
    Field::new("pgtables", 56, 8),
    Field::new("oom_score_adj", 64, 2),
];

/// One `field:` line of a tracepoint `format` file, reduced to what the decode depends on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Field {
    pub name: &'static str,
    pub offset: usize,
    pub size: usize,
}

impl Field {
    const fn new(name: &'static str, offset: usize, size: usize) -> Self {
        Self { name, offset, size }
    }
}

/// How the kernel's layout differs from the one the probe decodes.
#[derive(Debug, PartialEq, Eq)]
pub enum Mismatch {
    /// The field is absent — the pre-6.9 tracepoint, most likely.
    Missing { name: &'static str },
    /// The field exists but has moved or changed width.
    Moved {
        name: &'static str,
        expected: (usize, usize),
        found: (usize, usize),
    },
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { name } => write!(f, "  {name}: missing"),
            Self::Moved {
                name,
                expected: (eo, es),
                found: (fo, fs),
            } => write!(
                f,
                "  {name}: expected offset {eo} size {es}, found offset {fo} size {fs}"
            ),
        }
    }
}

/// Parse the `field:` lines of a tracepoint `format` file into `(name, offset, size)`.
///
/// Lines look like `\tfield:unsigned long total_vm;\toffset:16;\tsize:8;\tsigned:0;`. The
/// field name is the last token of the declaration, with any array suffix stripped, so both
/// `__data_loc char[] comm` and `char comm[16]` reduce to `comm`.
pub fn parse_format(text: &str) -> Vec<(String, usize, usize)> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("field:")?;
            let (decl, attrs) = rest.split_once(';')?;
            let name = decl
                .split_whitespace()
                .next_back()?
                .split('[')
                .next()?
                .trim_start_matches('*');
            if name.is_empty() {
                return None;
            }
            Some((
                name.to_string(),
                attr(attrs, "offset")?,
                attr(attrs, "size")?,
            ))
        })
        .collect()
}

/// Pull `<key>:<usize>` out of the trailing `offset:16;\tsize:8;\tsigned:0;` run.
fn attr(attrs: &str, key: &str) -> Option<usize> {
    attrs.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(key)?
            .strip_prefix(':')?
            .trim()
            .parse()
            .ok()
    })
}

/// Compare a `format` file against the layout the probe decodes.
pub fn check_layout(text: &str) -> Vec<Mismatch> {
    let found = parse_format(text);
    REQUIRED_FIELDS
        .iter()
        .filter_map(
            |want| match found.iter().find(|(name, _, _)| name == want.name) {
                None => Some(Mismatch::Missing { name: want.name }),
                Some((_, offset, size)) if (*offset, *size) != (want.offset, want.size) => {
                    Some(Mismatch::Moved {
                        name: want.name,
                        expected: (want.offset, want.size),
                        found: (*offset, *size),
                    })
                }
                Some(_) => None,
            },
        )
        .collect()
}

/// Read the `oom:mark_victim` `format` file from whichever tracefs mount exposes it.
fn read_format() -> Result<String> {
    let mut last_err: Option<(&str, io::Error)> = None;
    for path in FORMAT_PATHS {
        match fs::read_to_string(Path::new(path)) {
            Ok(text) => return Ok(text),
            Err(e) => last_err = Some((path, e)),
        }
    }

    let (path, err) = last_err.expect("FORMAT_PATHS is non-empty");
    Err(anyhow!(
        "could not read the oom:mark_victim tracepoint format ({path}: {err}).\n\
         Mount tracefs and make it visible to this container — the DaemonSet mounts \
         /sys and /sys/kernel/debug for exactly this."
    ))
}

/// Assert the running kernel's `oom:mark_victim` layout matches what the probe decodes,
/// before the probe is loaded. Returns an error naming every discrepancy.
pub fn verify_kernel_layout() -> Result<()> {
    let text = read_format()?;
    let mismatches = check_layout(&text);
    if mismatches.is_empty() {
        return Ok(());
    }

    let detail = mismatches
        .iter()
        .map(|m| m.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let all_missing = mismatches
        .iter()
        .all(|m| matches!(m, Mismatch::Missing { .. }));

    Err(anyhow!(
        "the oom:mark_victim tracepoint on this kernel does not match the layout this \
         probe decodes:\n{detail}\n\n{}\n\
         Refusing to start: attaching anyway would report correct PIDs alongside \
         meaningless memory figures.",
        if all_missing {
            "This is the pre-6.9 tracepoint, which carries only `pid`. The extended fields \
             landed in Linux 6.9; this node needs a 6.9+ kernel."
        } else {
            "The tracepoint layout has changed. MarkVictimArgs in oom-watcher-ebpf and \
             REQUIRED_FIELDS here both need updating for this kernel."
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim `/sys/kernel/tracing/events/oom/mark_victim/format` from a 6.9+ kernel.
    const MODERN: &str = "name: mark_victim
ID: 1442
format:
\tfield:unsigned short common_type;\toffset:0;\tsize:2;\tsigned:0;
\tfield:unsigned char common_flags;\toffset:2;\tsize:1;\tsigned:0;
\tfield:unsigned char common_preempt_count;\toffset:3;\tsize:1;\tsigned:0;
\tfield:int common_pid;\toffset:4;\tsize:4;\tsigned:1;

\tfield:int pid;\toffset:8;\tsize:4;\tsigned:1;
\tfield:__data_loc char[] comm;\toffset:12;\tsize:4;\tsigned:1;
\tfield:unsigned long total_vm;\toffset:16;\tsize:8;\tsigned:0;
\tfield:unsigned long anon_rss;\toffset:24;\tsize:8;\tsigned:0;
\tfield:unsigned long file_rss;\toffset:32;\tsize:8;\tsigned:0;
\tfield:unsigned long shmem_rss;\toffset:40;\tsize:8;\tsigned:0;
\tfield:unsigned int uid;\toffset:48;\tsize:4;\tsigned:0;
\tfield:unsigned long pgtables;\toffset:56;\tsize:8;\tsigned:0;
\tfield:short oom_score_adj;\toffset:64;\tsize:2;\tsigned:1;

print fmt: \"pid=%d comm=%s total-vm=%lukB\", REC->pid, __get_str(comm), REC->total_vm
";

    /// The pre-6.9 tracepoint: same name, same `common_*` header, only `pid` after it.
    const PRE_6_9: &str = "name: mark_victim
ID: 1189
format:
\tfield:unsigned short common_type;\toffset:0;\tsize:2;\tsigned:0;
\tfield:unsigned char common_flags;\toffset:2;\tsize:1;\tsigned:0;
\tfield:unsigned char common_preempt_count;\toffset:3;\tsize:1;\tsigned:0;
\tfield:int common_pid;\toffset:4;\tsize:4;\tsigned:1;

\tfield:int pid;\toffset:8;\tsize:4;\tsigned:1;

print fmt: \"pid=%d\", REC->pid
";

    #[test]
    fn parses_every_field_line() {
        let fields = parse_format(MODERN);
        // 4 common_* + 9 payload fields.
        assert_eq!(fields.len(), 13);
        assert!(fields.contains(&("total_vm".to_string(), 16, 8)));
        assert!(fields.contains(&("oom_score_adj".to_string(), 64, 2)));
    }

    #[test]
    fn reduces_data_loc_declaration_to_the_bare_name() {
        let fields = parse_format(MODERN);
        assert!(fields.contains(&("comm".to_string(), 12, 4)));
    }

    #[test]
    fn strips_array_suffix_from_field_name() {
        let fields = parse_format("\tfield:char comm[16];\toffset:12;\tsize:16;\tsigned:1;");
        assert_eq!(fields, vec![("comm".to_string(), 12, 16)]);
    }

    #[test]
    fn accepts_the_layout_the_probe_decodes() {
        assert_eq!(check_layout(MODERN), vec![]);
    }

    #[test]
    fn reports_every_field_the_pre_6_9_tracepoint_lacks() {
        // The load-bearing case: `pid` is present and correct, which is exactly why this
        // kernel would otherwise pass unnoticed and emit garbage memory figures.
        let mismatches = check_layout(PRE_6_9);
        assert_eq!(mismatches.len(), 8);
        assert!(mismatches
            .iter()
            .all(|m| matches!(m, Mismatch::Missing { .. })));
        assert!(mismatches.contains(&Mismatch::Missing { name: "total_vm" }));
        assert!(!mismatches.contains(&Mismatch::Missing { name: "pid" }));
    }

    #[test]
    fn reports_a_field_that_moved_rather_than_vanished() {
        let shifted = MODERN.replace(
            "field:unsigned long pgtables;\toffset:56;\tsize:8;",
            "field:unsigned long pgtables;\toffset:60;\tsize:8;",
        );
        assert_eq!(
            check_layout(&shifted),
            vec![Mismatch::Moved {
                name: "pgtables",
                expected: (56, 8),
                found: (60, 8),
            }]
        );
    }

    #[test]
    fn ignores_non_field_lines() {
        assert_eq!(
            parse_format("name: mark_victim\nID: 1442\nformat:\n"),
            vec![]
        );
    }
}
