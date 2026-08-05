//! Profile picker — thin wrapper around [`SearchablePicker`] for selecting
//! agent profiles.

use crate::widgets::searchable_picker::{PickerItem, PickerState, SearchablePicker};

/// One selectable profile entry.
#[derive(Clone)]
pub struct ProfileItem {
    pub name: String,
    pub description: String,
}

impl PickerItem for ProfileItem {
    fn title(&self) -> &str {
        &self.name
    }
    fn keywords(&self) -> &str {
        &self.description
    }
    fn description(&self) -> Option<&str> {
        Some(&self.description)
    }
}

/// State type alias.
pub type ProfilePickerState = PickerState<ProfileItem>;

impl Default for ProfilePickerState {
    fn default() -> Self {
        PickerState::new_empty()
    }
}

/// Set profile data on the picker state.
pub fn set_profiles(state: &mut ProfilePickerState, profiles: &[(String, String)]) {
    state.set_items(
        profiles
            .iter()
            .map(|(name, desc)| ProfileItem {
                name: name.clone(),
                description: desc.clone(),
            })
            .collect(),
    );
}

/// Render the profile picker popup.
pub fn render_profile_picker(
    frame_area: ratatui::layout::Rect,
    buf: &mut ratatui::prelude::Buffer,
    state: &mut ProfilePickerState,
) {
    SearchablePicker::new(" Select Profile ")
        .accent(ratatui::style::Color::Blue)
        .placeholder(" search profiles…")
        .footer_hint(" Enter spawn  ↑↓ navigate  Esc cancel")
        .render(frame_area, buf, state);
}
