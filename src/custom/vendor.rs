//! Custom provider → the shared vendor outcome.
//!
//! There is no Waybar renderer here, unlike the built-in vendors: a custom
//! provider's shape *is* the generic report shape, so the frontends
//! (`report.rs`, the TUI panels) render it from the snapshot directly.

use crate::usage::VendorSnapshot;
use crate::vendor::VendorOutcome;

use super::fetch::FetchOutcome;

impl From<FetchOutcome> for VendorOutcome {
    fn from(o: FetchOutcome) -> Self {
        o.map(VendorSnapshot::Custom)
    }
}
