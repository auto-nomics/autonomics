use std::{collections::HashMap, hash::Hash};

use agentik_sdk::{model::Model, provider::deepseek::DeepseekProvider};
use ratatui::{buffer::Buffer, layout::Rect, widgets::StatefulWidgetRef};

pub struct ModelConfigWidget {}

pub struct ModelConfigWidgetState {
    active_model: Model,
}

impl StatefulWidgetRef for ModelConfigWidget {
    type State = ModelConfigWidgetState;

    fn render_ref(&self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        todo!()
    }
}
