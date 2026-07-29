use std::{collections::HashMap, hash::Hash};

use agentik_sdk::provider::deepseek::DeepseekProvider;
use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidgetRef};

pub struct ModelConfigWidget {}

pub struct ModelConfigWidgetState {}

impl StatefulWidgetRef for ModelConfigWidget {
    type State = ModelConfigWidgetState;

    #[doc = " Draws the current state of the widget in the given buffer. That is the only method required"]
    #[doc = " to implement a custom stateful widget."]
    fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        todo!()
    }
}
