//! Progress reporting for a long push, at **asset** granularity (D6).
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

use crate::sync::report::{Style, human_bytes};

/// The one line both implementations render, as a pure function of the
/// counters.
///
/// **Counts and byte totals only.** The asset name reaches
/// [`Progress::asset_done`] and deliberately stops there: it is a content
/// address today, and a reporter that prints its argument is one refactor away
/// from printing something that is not (T-4-25).
pub fn render(done: usize, total: usize, bytes_done: u64, bytes_total: u64) -> String {
    format!(
        "uploading {done}/{total} asset{} — {} of {}",
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
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
    style: Style,
) -> String {
    if !style.is_on() {
        return render(done, total, bytes_done, bytes_total);
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
            "uploading {done}/{total} asset{} —",
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
    done: usize,
    total: usize,
    bytes_done: u64,
    bytes_total: u64,
}

impl Counters {
    fn start(&mut self, assets: usize, total_bytes: u64) {
        self.total = assets;
        self.bytes_total = total_bytes;
    }

    fn asset_done(&mut self, bytes: u64) {
        self.done += 1;
        self.bytes_done += bytes;
    }

    fn line(&self) -> String {
        render(self.done, self.total, self.bytes_done, self.bytes_total)
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

    fn rewrite(&mut self, line: &str) {
        let visible = visible_width(line);
        let pad = self.wide.saturating_sub(visible);
        self.wide = self.wide.max(visible);
        let _ = write!(self.out, "\r{line}{}", " ".repeat(pad));
        let _ = self.out.flush();
    }

    fn line(&self) -> String {
        render_styled(
            self.at.done,
            self.at.total,
            self.at.bytes_done,
            self.at.bytes_total,
            self.style,
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

    fn asset_done(&mut self, _index: usize, _name: &str, bytes: u64) {
        self.at.asset_done(bytes);
        self.rewrite(&self.line());
    }

    fn finish(&mut self) {
        let _ = writeln!(self.out);
        let _ = self.out.flush();
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

    fn asset_done(&mut self, _index: usize, _name: &str, bytes: u64) {
        self.at.asset_done(bytes);
        let _ = writeln!(self.out, "{}", self.at.line());
    }

    fn finish(&mut self) {
        let _ = writeln!(self.out, "{} — done", self.at.line());
        let _ = self.out.flush();
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
            render(0, 3, 0, 3 * 1024 * 1024),
            "uploading 0/3 assets — 0 B of 3.0 MiB"
        );
        assert_eq!(
            render(2, 3, 2 * 1024 * 1024, 3 * 1024 * 1024),
            "uploading 2/3 assets — 2.0 MiB of 3.0 MiB"
        );
        assert_eq!(
            render(1, 1, 512, 512),
            "uploading 1/1 asset — 512 B of 512 B"
        );
        // Pure: same arguments, same string, no clock and no environment.
        assert_eq!(render(1, 2, 5, 9), render(1, 2, 5, 9));
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
