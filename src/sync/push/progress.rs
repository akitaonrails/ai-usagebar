//! Progress reporting for a long push **or restore**, at **asset** granularity
//! (D6).
//!
//! A first push moves ~115 MB in a handful of large assets, so "asset i of n,
//! bytes done of total" is the whole of what a user needs. There is deliberately
//! no per-chunk hook and one must not be added: 5,000 chunk callbacks per push
//! is chatter, and the thing a user is waiting on is the upload, which happens
//! one asset at a time.
//!
//! Plan 4-01 defined the trait and [`Silent`]; plan 4-03 added [`Terminal`] and
//! [`Plain`] behind it, chosen by [`reporter`] from an **injected** flag.
//!
//! # One vocabulary, three stages
//!
//! Plan 6-12 gave the restore path the same reporter rather than a second one.
//! The shapes did not differ: a restore is also "n things of a known total size,
//! one at a time", so the only thing the download and the write needed was a
//! [`Stage`] — the verb and the noun — and the counters, the writer, the
//! `\r` rewrite, the padding and the `NO_COLOR` handling are shared verbatim.
//! [`UPLOAD`] reproduces the pre-6-12 push line byte for byte.
//!
//! Both are split into a pure renderer — [`render`] — and a thin writer, and
//! the tests assert against the renderer's string. A progress reporter whose
//! correctness can only be checked by looking at a terminal is a progress
//! reporter with no tests.
//!
//! Everything goes to **standard error**. `sync push`'s standard output is the
//! machine-readable outcome, and a progress line in it would corrupt anything
//! piping the command (T-4-27).

/// What `upload::run` reports as it works.
///
/// A trait rather than a closure so the non-terminal implementation can hold the
/// rate-limiting state D6 asks for, and so `upload::run` needs no branch.
pub trait Progress {
    /// A long **local** phase, before a byte goes on the wire.
    ///
    /// The upload is not the slow part of a first push. Deriving the Argon2id
    /// key, then reading and hashing the tree, then reading it again to seal it
    /// into packs, is where the minute goes — and until this existed the whole
    /// of it was a dead terminal under one `deriving the sync key…` line. Each
    /// call marks a transition, with the plan's own figures where the caller has
    /// them yet (`files == 0` means it does not).
    ///
    /// ponytail: transitions, not a bar. A moving bar here needs a per-file
    /// callback inside `plan::build` — 21 call sites — and inside
    /// `packer::build`; if the two reads stay this expensive, that is the
    /// upgrade, and this signature already carries the totals it would need.
    ///
    /// Defaulted to a no-op so every existing implementation — [`Silent`], and
    /// the recording doubles in `tests/sync_push_e2e.rs` — keeps compiling
    /// without knowing this exists.
    fn phase(&mut self, label: &str, files: usize, bytes: u64) {
        let _ = (label, files, bytes);
    }

    /// Called once, with the assets actually being uploaded and the sum of their
    /// lengths — measured, never projected.
    fn start(&mut self, assets: usize, total_bytes: u64);

    /// [`start`](Progress::start) for a run that has more than one stage.
    ///
    /// A restore has three — the key derivation (a [`phase`](Progress::phase)),
    /// the download and the write — and each needs its own verb, its own noun
    /// and its own counters. `start` is this with [`UPLOAD`], which is why push
    /// still calls `start` and its output is unchanged.
    ///
    /// Defaulted to `start` so [`Silent`] and the recording doubles in
    /// `tests/sync_push_e2e.rs` keep compiling without knowing this exists.
    fn stage(&mut self, stage: Stage, items: usize, total_bytes: u64) {
        let _ = stage;
        self.start(items, total_bytes);
    }

    /// One completed asset. `index` is zero-based.
    fn asset_done(&mut self, index: usize, name: &str, bytes: u64);

    /// Called once, whether the run succeeded or not.
    fn finish(&mut self);
}

/// No-ops. What every test passes, and what a caller that wants no output uses.
#[derive(Debug, Default)]
pub struct Silent;

impl Progress for Silent {
    fn start(&mut self, _assets: usize, _total_bytes: u64) {}
    fn asset_done(&mut self, _index: usize, _name: &str, _bytes: u64) {}
    fn finish(&mut self) {}
}

use std::io::{Stderr, Write, stderr};
use std::time::{Duration, Instant};

use crate::sync::report::{Style, human_bytes};

/// What one stretch of a run is called, and whether its remaining time can
/// honestly be projected.
///
/// The three constants below are the whole vocabulary; there is no fourth and a
/// caller does not invent one, because every word here appears on a user's
/// terminal and "downloading" and "fetching" for the same act is how two halves
/// of one tool start describing themselves differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage {
    /// The present participle: what is happening.
    pub verb: &'static str,
    /// The singular noun for one unit of it. Pluralised by [`render`].
    pub noun: &'static str,
    /// Whether [`eta`] may run on this stage's counters.
    ///
    /// **Not decoration, and not on by default.** It is true only where the
    /// remaining work is measured in the same unit as the rate: see [`DOWNLOAD`]
    /// and, for the two that deliberately decline it, [`UPLOAD`] and [`WRITE`].
    pub eta: bool,
}

impl Default for Stage {
    fn default() -> Self {
        UPLOAD
    }
}

/// `push::upload` — the pre-6-12 line, unchanged.
///
/// No ETA: an upload's rate is the same measurable thing a download's is, but
/// adding one here would change output the macOS menu bar and 6-06's pty test
/// already pin, for a stretch nobody reported waiting blind through. If it is
/// ever wanted, flipping this one flag is the whole change.
pub const UPLOAD: Stage = Stage {
    verb: "uploading",
    noun: "asset",
    eta: false,
};

/// `restore::fetch` — the ~880 MiB of packs a second machine waits on.
///
/// **The one stage with an ETA, and the only one that has earned it.** The
/// remaining bytes are the sizes the release listing declared before the first
/// request, and the rate is bytes actually off the wire over elapsed time —
/// two measurements, divided. Nothing is extrapolated from a guess.
///
/// ponytail: per **pack**, not per byte. `fetch::download` buffers a whole
/// asset through `Client::download_asset`, so there is no byte-level hook inside
/// one 32 MiB pack to report from and this deliberately does not pretend there
/// is: the bar steps by a pack at a time. Smoothness needs a streaming download
/// verb and `reqwest`'s `stream` feature — that is the upgrade, and this
/// signature already carries the totals it would need.
pub const DOWNLOAD: Stage = Stage {
    verb: "downloading",
    noun: "pack",
    eta: true,
};

/// `restore::write` — putting the manifest's items back on the disk.
///
/// No ETA. The bytes are known, but a write's cost is dominated by the
/// per-file syscalls, not the bytes: 400 small items and one 2 GiB one give a
/// measured "rate" that describes neither, and a projection off it would be a
/// number this code cannot stand behind. The count is honest and the count is
/// what is shown.
pub const WRITE: Stage = Stage {
    verb: "writing",
    noun: "item",
    eta: false,
};

/// Elapsed time, injected — so no test reads a clock.
///
/// [`Monotonic`](Clock::Monotonic) is `Instant`, which is monotonic rather than
/// wall time and so cannot run backwards over an NTP step mid-restore.
/// [`Fixed`](Clock::Fixed) is what a test passes, and is what makes every
/// assertion about an ETA in this file exact rather than approximate.
#[derive(Debug, Clone, Copy)]
pub enum Clock {
    Monotonic(Instant),
    Fixed(Duration),
}

impl Default for Clock {
    fn default() -> Self {
        Clock::Monotonic(Instant::now())
    }
}

impl Clock {
    fn elapsed(self) -> Duration {
        match self {
            Clock::Monotonic(since) => since.elapsed(),
            Clock::Fixed(d) => d,
        }
    }

    /// A new stage starts its own measurement: the download's rate must not be
    /// averaged with the seconds the key derivation spent.
    fn restart(self) -> Self {
        match self {
            Clock::Monotonic(_) => Clock::Monotonic(Instant::now()),
            fixed => fixed,
        }
    }
}

/// How long the rest of a stage will take, from the rate it has actually run
/// at — or `None` when there is no measurement to divide.
///
/// Refuses on all three of the cases where the arithmetic would produce a
/// confident number out of nothing: no bytes moved yet (no rate), nothing left
/// (no remainder), and under a second elapsed (a rate measured over a fraction
/// of a second and multiplied by 880 MiB is noise wearing a number's clothes).
pub fn eta(bytes_done: u64, bytes_total: u64, elapsed: Duration) -> Option<Duration> {
    if bytes_done == 0 || bytes_done >= bytes_total || elapsed < Duration::from_secs(1) {
        return None;
    }
    let left = u128::from(bytes_total - bytes_done);
    let secs = (left * elapsed.as_millis()) / u128::from(bytes_done) / 1000;
    Some(Duration::from_secs(secs.min(u128::from(u64::MAX)) as u64))
}

/// `45s`, `3m 20s`, `1h 04m`. Coarse on purpose: a projection accurate to the
/// second would be claiming a precision the rate does not have.
pub fn human_left(left: Duration) -> String {
    let secs = left.as_secs();
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {:02}s", secs / 60, secs % 60),
        _ => format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// The ETA's trailing clause, or nothing at all.
///
/// Kept apart from [`render`] so the two are testable — and refusable —
/// independently: a stage with no honest projection renders exactly the line it
/// rendered before this existed.
pub fn render_left(left: Option<Duration>, style: Style) -> String {
    match left {
        None => String::new(),
        Some(d) => format!(" {}", style.dim(&format!("— ~{} left", human_left(d)))),
    }
}

/// The one line both implementations render, as a pure function of the
/// counters.
///
/// **Counts and byte totals only.** The asset name reaches
/// [`Progress::asset_done`] and deliberately stops there: it is a content
/// address on the push side, an attacker-chosen manifest path on the restore
/// side, and a reporter that prints its argument is one refactor away from
/// printing either (T-4-25).
pub fn render(
    stage: Stage,
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
) -> String {
    format!(
        "{} {done}/{total} {}{} — {} of {}",
        stage.verb,
        stage.noun,
        if total == 1 { "" } else { "s" },
        human_bytes(bytes_done),
        human_bytes(bytes_total),
    )
}

/// Cells in the drawn bar. 24 keeps the whole styled line inside 80 columns
/// with the widest byte figures this tool produces.
const BAR: usize = 24;

/// `[████░░░░░░░░]` — the one thing the user asked for by name.
///
/// Pure, and bounded: a `bytes_done` above `bytes_total` (which cannot happen,
/// but is one arithmetic slip away) fills the bar rather than panicking on a
/// negative repeat count.
pub fn bar(bytes_done: u64, bytes_total: u64) -> String {
    let filled = if bytes_total == 0 {
        0
    } else {
        ((u128::from(bytes_done) * BAR as u128) / u128::from(bytes_total)) as usize
    }
    .min(BAR);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(BAR - filled))
}

/// [`render`] with the bar and the palette.
///
/// **[`Style::PLAIN`] returns [`render`] verbatim**, bar and all omitted: the
/// plain reporter is what a pipe, a log file and the macOS menu bar read, and
/// `NO_COLOR` lands here too — someone who asked for no decoration is not asking
/// for a box-drawing bar either.
pub fn render_styled(
    stage: Stage,
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
    style: Style,
) -> String {
    if !style.is_on() {
        return render(stage, done, total, bytes_done, bytes_total);
    }
    let pct = if bytes_total == 0 {
        0
    } else {
        (u128::from(bytes_done) * 100 / u128::from(bytes_total)) as u64
    };
    format!(
        "{} {} {} {}",
        style.dim(&bar(bytes_done, bytes_total)),
        style.bold(&format!("{pct:>3}%")),
        style.dim(&format!(
            "{} {done}/{total} {}{} —",
            stage.verb,
            stage.noun,
            if total == 1 { "" } else { "s" }
        )),
        style.bold(&format!(
            "{} of {}",
            human_bytes(bytes_done),
            human_bytes(bytes_total)
        )),
    )
}

/// The line a [`Progress::phase`] call draws.
///
/// `files == 0` is "no figures yet" rather than "no files": the first phase of a
/// push starts before the planner has walked anything, and a confident `0 files`
/// there would be a lie about the tree rather than an admission about the clock.
pub fn render_phase(label: &str, files: usize, bytes: u64, style: Style) -> String {
    if files == 0 {
        return format!("{}…", style.dim(label));
    }
    format!(
        "{} {} {} {}",
        style.dim(label),
        style.bold(&files.to_string()),
        style.dim(&format!("file{} —", if files == 1 { "" } else { "s" })),
        style.bold(&human_bytes(bytes)),
    )
}

/// What both implementations carry, so the arithmetic exists once.
#[derive(Debug, Default, Clone, Copy)]
struct Counters {
    stage: Stage,
    clock: Clock,
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
}

impl Counters {
    fn start(&mut self, assets: usize, total_bytes: u64) {
        self.total = assets;
        self.bytes_total = total_bytes;
        self.clock = self.clock.restart();
    }

    /// A new stage keeps the injected clock and drops every counter: the
    /// download's percentage must not carry the metadata round's bytes.
    fn begin(&mut self, stage: Stage, items: usize, total_bytes: u64) {
        *self = Counters {
            stage,
            clock: self.clock,
            ..Counters::default()
        };
        self.start(items, total_bytes);
    }

    fn asset_done(&mut self, bytes: u64) {
        self.done += 1;
        self.bytes_done += bytes;
    }

    /// The projection, if this stage has one to make.
    fn left(&self) -> Option<Duration> {
        if !self.stage.eta {
            return None;
        }
        eta(self.bytes_done, self.bytes_total, self.clock.elapsed())
    }

    fn line(&self) -> String {
        format!(
            "{}{}",
            render(
                self.stage,
                self.done,
                self.total,
                self.bytes_done,
                self.bytes_total
            ),
            render_left(self.left(), Style::PLAIN),
        )
    }
}

/// One line, rewritten in place — for a terminal.
///
/// Generic over its sink so a test can assert against the bytes actually
/// emitted rather than against a tty. Write failures are **swallowed**: a
/// closed standard error must not fail a push that is otherwise succeeding.
#[derive(Debug)]
pub struct Terminal<W = Stderr> {
    out: W,
    at: Counters,
    style: Style,
    /// The visible width of the widest line written so far.
    ///
    /// A `\r` rewrite only overwrites what it covers, so a line that gets
    /// *shorter* leaves the tail of its predecessor on screen — `0 B of 300 B`
    /// following `24.0 MiB of 300 B` used to leave a stray `B`. Padding to the
    /// high-water mark erases it without `\x1b[K`, which matters: `NO_COLOR`
    /// takes the styled writer too, and it must emit no escapes at all.
    wide: usize,
}

impl Default for Terminal<Stderr> {
    fn default() -> Self {
        Self::to(stderr())
    }
}

impl<W: Write> Terminal<W> {
    pub fn to(out: W) -> Self {
        Self {
            out,
            at: Counters::default(),
            style: Style::PLAIN,
            wide: 0,
        }
    }

    /// The palette, injected — this module may not read the environment.
    pub fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// The clock, injected — so a test can assert an exact ETA.
    pub fn clocked(mut self, clock: Clock) -> Self {
        self.at.clock = clock;
        self
    }

    /// End whatever bar is on screen, once. Idempotent, and a no-op when
    /// nothing was ever drawn — `sync pull` runs `restore::run` twice and a
    /// blank line between the two passes would be the only thing that marked it.
    fn end_line(&mut self) {
        if self.at.total == 0 {
            return;
        }
        let _ = writeln!(self.out);
        let _ = self.out.flush();
        self.at = Counters {
            clock: self.at.clock,
            ..Counters::default()
        };
        self.wide = 0;
    }

    fn rewrite(&mut self, line: &str) {
        let visible = visible_width(line);
        let pad = self.wide.saturating_sub(visible);
        self.wide = self.wide.max(visible);
        let _ = write!(self.out, "\r{line}{}", " ".repeat(pad));
        let _ = self.out.flush();
    }

    fn line(&self) -> String {
        format!(
            "{}{}",
            render_styled(
                self.at.stage,
                self.at.done,
                self.at.total,
                self.at.bytes_done,
                self.at.bytes_total,
                self.style,
            ),
            render_left(self.at.left(), self.style),
        )
    }
}

impl<W: Write> Progress for Terminal<W> {
    /// A completed step, kept on screen: the bar below it rewrites its own line
    /// and would otherwise erase the only evidence of the minute that preceded
    /// it.
    fn phase(&mut self, label: &str, files: usize, bytes: u64) {
        self.rewrite(&render_phase(label, files, bytes, self.style));
        let _ = writeln!(self.out);
        let _ = self.out.flush();
        self.wide = 0;
    }

    fn start(&mut self, assets: usize, total_bytes: u64) {
        self.at.start(assets, total_bytes);
        self.rewrite(&self.line());
    }

    /// Ends the previous stage's line before beginning this one's, for the same
    /// reason [`phase`](Progress::phase) does: the bar rewrites its own line, so
    /// without this the completed download would be erased by the write that
    /// follows it and a finished restore would show only its last stage.
    fn stage(&mut self, stage: Stage, items: usize, total_bytes: u64) {
        self.end_line();
        self.at.begin(stage, items, total_bytes);
        self.rewrite(&self.line());
    }

    fn asset_done(&mut self, _index: usize, _name: &str, bytes: u64) {
        self.at.asset_done(bytes);
        self.rewrite(&self.line());
    }

    /// Leaves the cursor at column 0 of a fresh line — **on every path**,
    /// including a failed one. `restore::run` calls this whether or not the run
    /// returned `Ok`, because otherwise the error message that follows starts in
    /// the middle of the bar.
    fn finish(&mut self) {
        self.end_line();
    }
}

/// Columns a line occupies, which is not its byte length once it carries SGR
/// sequences. Only the two forms this module emits are recognised — `ESC [ …
/// m` — because those are the only ones [`Style`] produces.
fn visible_width(line: &str) -> usize {
    let mut width = 0usize;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for skip in chars.by_ref() {
                if skip == 'm' {
                    break;
                }
            }
            continue;
        }
        width += 1;
    }
    width
}

/// One plain line per completed asset — for anything that is not a terminal.
///
/// D6 is explicit that non-TTY output degrades to periodic lines and never a
/// spinner, because the macOS menu bar captures this command's output as a
/// subprocess and carriage returns and escape sequences make that capture
/// unreadable. The cadence is one line per asset, which at `PACK_MAX` bodies is
/// already the right rate — there is no timer here and no second knob.
#[derive(Debug)]
pub struct Plain<W = Stderr> {
    out: W,
    at: Counters,
}

impl Default for Plain<Stderr> {
    fn default() -> Self {
        Self::to(stderr())
    }
}

impl<W: Write> Plain<W> {
    pub fn to(out: W) -> Self {
        Self {
            out,
            at: Counters::default(),
        }
    }

    /// The clock, injected — see [`Terminal::clocked`].
    pub fn clocked(mut self, clock: Clock) -> Self {
        self.at.clock = clock;
        self
    }
}

impl<W: Write> Progress for Plain<W> {
    /// Plain too — a capture that shows nothing for a minute is exactly as
    /// unreadable as a terminal that does, and this costs one line per phase.
    /// Never styled: [`Style::PLAIN`] is what makes that structural.
    fn phase(&mut self, label: &str, files: usize, bytes: u64) {
        let _ = writeln!(
            self.out,
            "{}",
            render_phase(label, files, bytes, Style::PLAIN)
        );
    }

    fn start(&mut self, assets: usize, total_bytes: u64) {
        self.at.start(assets, total_bytes);
        let _ = writeln!(self.out, "{}", self.at.line());
    }

    fn stage(&mut self, stage: Stage, items: usize, total_bytes: u64) {
        self.at.begin(stage, items, total_bytes);
        let _ = writeln!(self.out, "{}", self.at.line());
    }

    fn asset_done(&mut self, _index: usize, _name: &str, bytes: u64) {
        self.at.asset_done(bytes);
        let _ = writeln!(self.out, "{}", self.at.line());
    }

    /// Nothing started, nothing to summarise — a restore that failed before its
    /// first request must not leave `0/0 — done` in a captured log.
    fn finish(&mut self) {
        if self.at.total == 0 {
            return;
        }
        let _ = writeln!(self.out, "{} — done", self.at.line());
        let _ = self.out.flush();
        self.at = Counters {
            clock: self.at.clock,
            ..Counters::default()
        };
    }
}

/// The reporter for this run, chosen from an **injected** flag.
///
/// `is_terminal` is supplied by the CLI from `std::io::IsTerminal` at the one
/// production call site, and `style` from
/// [`crate::display::color_enabled`] at the same one. Neither is read here: an
/// ambient environment read inside a constructor is exactly what makes a test
/// non-hermetic, and this project's convention is to inject the fact — the same
/// reason `Cache::at` exists beside `Cache::for_vendor`. For `NO_COLOR` it is
/// also structural: `src/sync/`'s guard forbids `std::env` in this subtree.
pub fn reporter(is_terminal: bool, style: Style) -> Box<dyn Progress> {
    if is_terminal {
        Box::new(Terminal::default().styled(style))
    } else {
        Box::new(Plain::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(reporter: &mut dyn Progress) {
        reporter.start(3, 300);
        reporter.asset_done(0, "pack-aa.bin", 100);
        reporter.asset_done(1, "pack-bb.bin", 100);
        reporter.finish();
    }

    #[test]
    fn the_renderer_is_a_pure_function_of_the_counters() {
        assert_eq!(
            render(UPLOAD, 0, 3, 0, 3 * 1024 * 1024),
            "uploading 0/3 assets — 0 B of 3.0 MiB"
        );
        assert_eq!(
            render(UPLOAD, 2, 3, 2 * 1024 * 1024, 3 * 1024 * 1024),
            "uploading 2/3 assets — 2.0 MiB of 3.0 MiB"
        );
        assert_eq!(
            render(UPLOAD, 1, 1, 512, 512),
            "uploading 1/1 asset — 512 B of 512 B"
        );
        // Pure: same arguments, same string, no clock and no environment.
        assert_eq!(render(UPLOAD, 1, 2, 5, 9), render(UPLOAD, 1, 2, 5, 9));
    }

    /// 6-12 gave the restore path this reporter rather than a second one, and
    /// the *only* thing that varies between the three stages is the two words.
    #[test]
    fn the_three_stages_differ_by_a_verb_and_a_noun_and_nothing_else() {
        assert_eq!(
            render(DOWNLOAD, 3, 12, 100 * 1024 * 1024, 880 * 1024 * 1024),
            "downloading 3/12 packs — 100.0 MiB of 880.0 MiB"
        );
        assert_eq!(
            render(
                WRITE,
                431,
                431,
                2 * 1024 * 1024 * 1024,
                2 * 1024 * 1024 * 1024
            ),
            "writing 431/431 items — 2.0 GiB of 2.0 GiB"
        );
        assert_eq!(
            render(DOWNLOAD, 1, 1, 5, 5),
            "downloading 1/1 pack — 5 B of 5 B"
        );
        // `start` is `stage(UPLOAD, …)`, which is what keeps push's own line
        // byte-identical to what it printed before this parameter existed.
        assert_eq!(Stage::default(), UPLOAD);
    }

    #[test]
    fn the_terminal_reporter_rewrites_one_line_and_ends_it_once() {
        let mut out = Vec::new();
        drive(&mut Terminal::to(&mut out));
        let written = String::from_utf8(out).unwrap();

        assert_eq!(
            written,
            "\ruploading 0/3 assets — 0 B of 300 B\
             \ruploading 1/3 assets — 100 B of 300 B\
             \ruploading 2/3 assets — 200 B of 300 B\n",
            "unstyled by default, and byte-for-byte what it always was"
        );
        assert_eq!(written.matches('\n').count(), 1, "one newline, from finish");
    }

    /// A `\r` rewrite only covers what it writes, so a line that gets *shorter*
    /// used to leave the tail of its predecessor on screen — and these lines do
    /// shrink, because `1023.9 KiB` is ten columns and the `1.0 MiB` that
    /// follows it is seven.
    #[test]
    fn a_shrinking_line_leaves_no_tail_of_the_one_before_it() {
        let mut out = Vec::new();
        let mut term = Terminal::to(&mut out);
        term.start(2, 2 * 1024 * 1024);
        term.asset_done(0, "a", 1023 * 1024);
        term.asset_done(1, "b", 1024 * 1024 + 1024);
        term.finish();
        let written = String::from_utf8(out).unwrap();

        // `trim_end` would strip the very padding under test.
        let widths: Vec<usize> = written
            .trim_end_matches('\n')
            .split('\r')
            .filter(|s| !s.is_empty())
            .map(visible_width)
            .collect();
        assert!(widths.len() == 3, "{written:?}");
        assert!(
            widths.windows(2).all(|w| w[1] >= w[0]),
            "no rewrite is narrower than the one it covers: {widths:?} in {written:?}"
        );
        assert!(
            !written.contains("\x1b["),
            "padded with spaces, never an erase"
        );
    }

    /// The reported defect: a first push spends its longest minute before the
    /// uploader is reached, and until `phase` existed that minute was a dead
    /// terminal.
    #[test]
    fn the_local_phases_before_the_upload_are_narrated_with_their_own_figures() {
        assert_eq!(
            render_phase(
                "sealing into packs",
                3800,
                2 * 1024 * 1024 * 1024,
                Style::PLAIN
            ),
            "sealing into packs 3800 files — 2.0 GiB"
        );
        assert_eq!(
            render_phase("reading what changed", 0, 0, Style::PLAIN),
            "reading what changed…",
            "no figures yet is an admission about the clock, never `0 files`"
        );
        assert_eq!(render_phase("one", 1, 5, Style::PLAIN), "one 1 file — 5 B");

        let mut out = Vec::new();
        let mut plain = Plain::to(&mut out);
        plain.phase("sealing into packs", 2, 4096);
        plain.start(1, 4096);
        plain.finish();
        let written = String::from_utf8(out).unwrap();
        assert!(written.starts_with("sealing into packs 2 files — 4.0 KiB\n"));
        assert!(!written.contains('\x1b'), "still plain: {written:?}");
    }

    #[test]
    fn the_bar_fills_with_the_bytes_and_is_bounded_at_both_ends() {
        assert_eq!(bar(0, 300), format!("[{}]", "░".repeat(BAR)));
        assert_eq!(bar(300, 300), format!("[{}]", "█".repeat(BAR)));
        assert_eq!(
            bar(150, 300),
            format!("[{}{}]", "█".repeat(12), "░".repeat(12))
        );
        // Neither of these can happen; neither may panic on a repeat count.
        assert_eq!(bar(0, 0), format!("[{}]", "░".repeat(BAR)));
        assert_eq!(bar(9, 3), format!("[{}]", "█".repeat(BAR)));
    }

    /// The styled path, driven through the same writer seam — no tty involved.
    #[test]
    fn the_styled_terminal_draws_a_bar_and_leaves_the_terminal_reset() {
        let mut out = Vec::new();
        drive(&mut Terminal::to(&mut out).styled(Style::color(true)));
        let written = String::from_utf8(out).unwrap();

        assert!(written.contains('█'), "a bar: {written:?}");
        assert!(written.contains("\x1b[2m"), "dim context: {written:?}");
        assert!(written.contains("\x1b[1m"), "bold figures: {written:?}");
        let closes = written.matches("\x1b[0m").count();
        assert_eq!(
            written.matches("\x1b[").count(),
            closes * 2,
            "every sequence opened is closed, so a dying push leaves no tint"
        );
        assert_eq!(
            written.matches('\n').count(),
            1,
            "still one line, rewritten"
        );
        assert!(!written.contains("\x1b[?"), "no cursor hiding: {written:?}");
        assert!(!written.contains("\x1b[2J"), "no alternate screen");
    }

    /// `NO_COLOR` reaches this module as [`Style::PLAIN`], and the styled writer
    /// then emits no escape at all — not a colour, not an erase-to-end-of-line,
    /// which is why `rewrite` pads with spaces instead.
    #[test]
    fn no_color_leaves_not_one_escape_sequence_in_the_styled_writer() {
        let no_color = Style::color(crate::display::color_enabled_with(true, true));
        assert_eq!(no_color, Style::PLAIN);

        let mut out = Vec::new();
        let mut term = Terminal::to(&mut out).styled(no_color);
        term.phase("sealing into packs", 3, 300);
        drive(&mut term);
        let written = String::from_utf8(out).unwrap();
        assert!(!written.contains('\x1b'), "{written:?}");
        assert!(!written.contains('█'), "{written:?}");
    }

    /// Padding is computed in columns, not bytes: an SGR sequence occupies no
    /// cell, and counting its bytes over-measures every styled line.
    #[test]
    fn the_rewrite_width_counts_cells_and_not_escape_bytes() {
        assert_eq!(visible_width("abc"), 3);
        assert_eq!(visible_width("\x1b[2mabc\x1b[0m"), 3);
        assert_eq!(visible_width("\x1b[1;36m—\x1b[0m"), 1);
    }

    /// D6: the macOS menu bar captures this command's output as a subprocess,
    /// and a carriage return or an escape sequence makes that capture
    /// unreadable.
    #[test]
    fn the_non_terminal_reporter_emits_plain_lines_and_nothing_else() {
        let mut out = Vec::new();
        drive(&mut Plain::to(&mut out));
        let written = String::from_utf8(out).unwrap();

        assert!(!written.contains('\r'), "no carriage returns: {written:?}");
        assert!(
            !written.contains('\x1b'),
            "no escape sequences: {written:?}"
        );
        assert!(written.ends_with('\n'));
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 4, "one per asset, plus start and the summary");
        assert_eq!(lines[2], "uploading 2/3 assets — 200 B of 300 B");
        assert!(
            lines[3].contains("done"),
            "a final summary line: {:?}",
            lines[3]
        );
    }

    /// T-4-25: the counters are the whole of what is rendered. The asset name
    /// reaches `asset_done` and must not reach the output — it is a content
    /// address today, and a reporter that prints its argument is one refactor
    /// away from printing something else.
    #[test]
    fn nothing_the_uploader_passes_by_name_reaches_the_output() {
        let secret = "github_pat_not_a_real_token";
        let mut out = Vec::new();
        let mut plain = Plain::to(&mut out);
        plain.start(1, 10);
        plain.asset_done(0, secret, 10);
        plain.finish();
        let written = String::from_utf8(out).unwrap();
        assert!(!written.contains(secret), "{written}");
        assert!(!written.contains("github_pat"), "{written}");
    }

    /// The reported defect on the inbound side: 880 MiB of packs arriving under
    /// a dead terminal. Driven through the writer seam, with the clock injected,
    /// so the assertion is exact and no test reads a real one.
    #[test]
    fn a_restores_stages_each_end_their_own_line_and_the_download_carries_an_eta() {
        let mut out = Vec::new();
        let mut term = Terminal::to(&mut out).clocked(Clock::Fixed(Duration::from_secs(10)));
        term.phase("deriving the sync key (Argon2id)", 0, 0);
        term.stage(DOWNLOAD, 2, 400);
        term.asset_done(0, "pack-aa.bin", 100);
        term.stage(WRITE, 2, 50);
        term.asset_done(0, "claude-home/settings.json", 25);
        term.finish();
        let written = String::from_utf8(out).unwrap();

        let lines: Vec<&str> = written.trim_end_matches('\n').split('\n').collect();
        assert_eq!(lines.len(), 3, "one per stage, kept: {written:?}");
        assert!(lines[0].ends_with("deriving the sync key (Argon2id)…"));
        // 100 B in 10 s is 10 B/s; the 300 B left are 30 s. Measured, divided.
        assert!(
            lines[1].ends_with("downloading 1/2 packs — 100 B of 400 B — ~30s left"),
            "{:?}",
            lines[1]
        );
        assert!(
            lines[2].ends_with("writing 1/2 items — 25 B of 50 B"),
            "no projection on a stage whose remaining work is syscalls: {:?}",
            lines[2]
        );
        assert!(!written.contains("\x1b[?"), "no cursor hiding");
        assert!(!written.contains("\x1b[2J"), "no alternate screen");
        assert!(
            written.ends_with('\n'),
            "the terminal is left on a fresh line"
        );
    }

    /// The three refusals. A number invented out of no measurement is worse
    /// than no number, and this milestone has shipped enough text asserting
    /// more than the code knew.
    #[test]
    fn an_eta_is_refused_wherever_there_is_no_rate_to_divide() {
        let ten = Duration::from_secs(10);
        assert_eq!(eta(0, 400, ten), None, "no bytes moved is no rate");
        assert_eq!(eta(400, 400, ten), None, "nothing left to project");
        assert_eq!(eta(500, 400, ten), None, "and it never goes negative");
        assert_eq!(
            eta(100, 400, Duration::from_millis(999)),
            None,
            "a rate measured over a fraction of a second, times 880 MiB, is noise"
        );
        assert_eq!(eta(100, 400, ten), Some(Duration::from_secs(30)));
        assert_eq!(
            eta(1, 1_000_001, ten),
            Some(Duration::from_secs(10_000_000))
        );

        assert_eq!(human_left(Duration::from_secs(45)), "45s");
        assert_eq!(human_left(Duration::from_secs(200)), "3m 20s");
        assert_eq!(human_left(Duration::from_secs(3864)), "1h 04m");
        assert_eq!(render_left(None, Style::PLAIN), "");
        assert_eq!(
            render_left(Some(Duration::from_secs(45)), Style::PLAIN),
            " — ~45s left"
        );
    }

    /// The constraint the macOS menu bar and the Node contract suites impose:
    /// a redirected run gets plain lines on stderr and a stdout nothing here
    /// ever touches.
    #[test]
    fn a_piped_restore_gets_the_plain_shape_and_no_escape_bytes() {
        let mut out = Vec::new();
        let mut plain = Plain::to(&mut out).clocked(Clock::Fixed(Duration::from_secs(10)));
        plain.phase("deriving the sync key (Argon2id)", 0, 0);
        plain.stage(DOWNLOAD, 2, 400);
        plain.asset_done(0, "pack-aa.bin", 100);
        plain.finish();
        let written = String::from_utf8(out).unwrap();

        assert!(!written.contains('\r'), "no carriage returns: {written:?}");
        assert!(!written.contains('\x1b'), "no escapes: {written:?}");
        assert!(!written.contains('█'), "no bar: {written:?}");
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines[0], "deriving the sync key (Argon2id)…");
        assert_eq!(lines[1], "downloading 0/2 packs — 0 B of 400 B");
        assert_eq!(
            lines[2],
            "downloading 1/2 packs — 100 B of 400 B — ~30s left"
        );
    }

    /// `NO_COLOR` on a real terminal still takes the styled writer, and the ETA
    /// clause has to obey it too — it is the one part of the line 6-12 added,
    /// and it goes through the same [`Style`] as everything before it.
    #[test]
    fn no_color_reaches_the_eta_clause_as_well_as_the_bar() {
        let mut out = Vec::new();
        let mut term = Terminal::to(&mut out)
            .styled(Style::color(crate::display::color_enabled_with(true, true)))
            .clocked(Clock::Fixed(Duration::from_secs(10)));
        term.stage(DOWNLOAD, 2, 400);
        term.asset_done(0, "pack-aa.bin", 100);
        term.finish();
        let written = String::from_utf8(out).unwrap();

        assert!(written.contains("~30s left"), "{written:?}");
        assert!(!written.contains('\x1b'), "{written:?}");
        assert!(!written.contains('█'), "{written:?}");
    }

    /// T-4-25 again, for the stage the restore added. A manifest path is
    /// **attacker-chosen** — it comes off a remote this format treats as
    /// hostile — so `write::apply` passing item names through `asset_done` is
    /// safe only for as long as no reporter prints its argument.
    #[test]
    fn an_attacker_chosen_manifest_path_never_reaches_a_progress_line() {
        let hostile = "claude-home/\x1b[2J\x07../../etc/passwd github_pat_not_a_real_token";
        for (label, written) in [
            ("terminal", {
                let mut out = Vec::new();
                let mut term = Terminal::to(&mut out).styled(Style::color(true));
                term.stage(WRITE, 1, 10);
                term.asset_done(0, hostile, 10);
                term.finish();
                String::from_utf8(out).unwrap()
            }),
            ("plain", {
                let mut out = Vec::new();
                let mut plain = Plain::to(&mut out);
                plain.stage(WRITE, 1, 10);
                plain.asset_done(0, hostile, 10);
                plain.finish();
                String::from_utf8(out).unwrap()
            }),
        ] {
            assert!(!written.contains("passwd"), "{label}: {written:?}");
            assert!(!written.contains("github_pat"), "{label}: {written:?}");
            assert!(!written.contains("\x1b[2J"), "{label}: {written:?}");
            assert!(!written.contains('\x07'), "{label}: {written:?}");
        }
    }

    /// The choice is an injected flag, never an ambient `IsTerminal` read: the
    /// project's convention is to inject the fact so the test is hermetic.
    #[test]
    fn the_reporter_is_chosen_from_an_injected_flag() {
        let mut tty = reporter(true, Style::color(true));
        let mut piped = reporter(false, Style::color(true));
        // Both satisfy the same trait, which is why `upload::run` needs no
        // branch. Driving them proves neither panics on the real streams.
        tty.start(0, 0);
        tty.finish();
        piped.start(0, 0);
        piped.finish();
    }
}
