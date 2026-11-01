//! Interactive TUI launch command.

use crate::cli::TuiArgs;

pub fn run_tui(_args: TuiArgs) -> color_eyre::Result<()> {
    autonomics_tui::run_tui()
}
