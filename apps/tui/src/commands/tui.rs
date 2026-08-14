//! Interactive TUI launch command.

use crate::app::App;
use crate::cli::TuiArgs;

pub fn run_tui(_args: TuiArgs) -> color_eyre::Result<()> {
    let mut app = App::new();
    app.start()
}
