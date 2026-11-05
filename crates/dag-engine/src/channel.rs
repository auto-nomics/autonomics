//! Runtime input channels and process firing semantics.
//!
//! Queue inputs consume one value per firing. Value inputs retain their value
//! for every firing. A process with only value inputs fires once. An unbound
//! closed input, or an exhausted closed queue, prevents further firings.
//! Each subscriber owns its own input buffer, permitting the explicitly
//! supported multi-reader wiring without competing consumers.

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Queue,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelError {
    Closed,
    AlreadyBound,
    InvalidInput,
}

#[derive(Debug)]
struct Input<T> {
    kind: ChannelKind,
    queue: VecDeque<T>,
    value: Option<T>,
    closed: bool,
    value_taken: bool,
}

/// An independently buffered subscriber for each input of one process.
#[derive(Debug)]
pub struct InputSet<T> {
    inputs: Vec<Input<T>>,
    scalar_fired: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Firing<T> {
    Ready(Vec<T>),
    Pending,
    Closed,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AnyFiring<T> {
    Ready(usize, T),
    Pending,
    Closed,
}

impl<T: Clone> InputSet<T> {
    pub fn new(kinds: impl IntoIterator<Item = ChannelKind>) -> Self {
        Self {
            inputs: kinds
                .into_iter()
                .map(|kind| Input {
                    kind,
                    queue: VecDeque::new(),
                    value: None,
                    closed: false,
                    value_taken: false,
                })
                .collect(),
            scalar_fired: false,
        }
    }

    /// Deliver one value. A value channel may be bound only once.
    pub fn send(&mut self, port: usize, value: T) -> Result<(), ChannelError> {
        let input = self
            .inputs
            .get_mut(port)
            .ok_or(ChannelError::InvalidInput)?;
        if input.closed {
            return Err(ChannelError::Closed);
        }
        match input.kind {
            ChannelKind::Queue => input.queue.push_back(value),
            ChannelKind::Value if input.value.is_none() => input.value = Some(value),
            ChannelKind::Value => return Err(ChannelError::AlreadyBound),
        }
        Ok(())
    }

    pub fn close(&mut self, port: usize) -> Result<(), ChannelError> {
        let input = self
            .inputs
            .get_mut(port)
            .ok_or(ChannelError::InvalidInput)?;
        input.closed = true;
        Ok(())
    }

    /// Queue capacity check for an individual subscriber. Value inputs are
    /// one-shot and do not grow with repeated deliveries.
    pub fn can_accept(&self, port: usize, capacity: usize) -> bool {
        self.inputs.get(port).is_some_and(|input| {
            !input.closed
                && match input.kind {
                    ChannelKind::Queue => input.queue.len() < capacity,
                    ChannelKind::Value => input.value.is_none(),
                }
        })
    }

    /// A tuple consumer may need another input before it can drain this one.
    pub(crate) fn waiting_on_other(&self, port: usize) -> bool {
        self.inputs.iter().enumerate().any(|(index, input)| {
            index != port
                && !input.closed
                && match input.kind {
                    ChannelKind::Queue => input.queue.is_empty(),
                    ChannelKind::Value => input.value.is_none(),
                }
        })
    }

    /// Drop buffered values when the process can no longer fire.
    pub fn discard(&mut self) {
        for input in &mut self.inputs {
            input.queue.clear();
            input.value = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn buffered_len(&self) -> usize {
        self.inputs
            .iter()
            .map(|input| input.queue.len() + usize::from(input.value.is_some()))
            .sum()
    }

    /// Consume one available input without waiting for the other channels.
    /// Used by mix; value channels contribute at most once.
    pub fn next_any(&mut self) -> AnyFiring<T> {
        for (index, input) in self.inputs.iter_mut().enumerate() {
            let value = match input.kind {
                ChannelKind::Queue => input.queue.pop_front(),
                ChannelKind::Value if !input.value_taken => {
                    let value = input.value.clone();
                    if value.is_some() {
                        input.value_taken = true;
                    }
                    value
                }
                ChannelKind::Value => None,
            };
            if let Some(value) = value {
                return AnyFiring::Ready(index, value);
            }
        }
        if self.inputs.iter().all(|input| match input.kind {
            ChannelKind::Queue => input.closed && input.queue.is_empty(),
            ChannelKind::Value => input.value_taken || (input.closed && input.value.is_none()),
        }) {
            AnyFiring::Closed
        } else {
            AnyFiring::Pending
        }
    }

    /// Consume channels in the caller-specified order, advancing only after
    /// the current channel has closed and its buffered items have drained.
    pub fn next_ordered(&mut self, order: &[usize]) -> AnyFiring<T> {
        for &index in order {
            let input = &mut self.inputs[index];
            let value = match input.kind {
                ChannelKind::Queue => input.queue.pop_front(),
                ChannelKind::Value if !input.value_taken => {
                    let value = input.value.clone();
                    if value.is_some() {
                        input.value_taken = true;
                    }
                    value
                }
                ChannelKind::Value => None,
            };
            if let Some(value) = value {
                return AnyFiring::Ready(index, value);
            }
            let complete = match input.kind {
                ChannelKind::Queue => input.closed,
                ChannelKind::Value => input.value_taken || input.closed,
            };
            if !complete {
                return AnyFiring::Pending;
            }
        }
        AnyFiring::Closed
    }

    /// Consume a complete tuple only when every input is ready. Closed queues
    /// are drained before reporting closure; no input is consumed on Pending.
    pub fn next(&mut self) -> Firing<T> {
        let has_queue = self.inputs.iter().any(|i| i.kind == ChannelKind::Queue);
        if !has_queue && self.scalar_fired {
            return Firing::Closed;
        }
        if self.inputs.iter().any(|i| {
            i.closed
                && match i.kind {
                    ChannelKind::Queue => i.queue.is_empty(),
                    ChannelKind::Value => i.value.is_none(),
                }
        }) {
            return Firing::Closed;
        }
        if self.inputs.iter().any(|i| match i.kind {
            ChannelKind::Queue => i.queue.is_empty(),
            ChannelKind::Value => i.value.is_none(),
        }) {
            return Firing::Pending;
        }
        self.scalar_fired = true;
        Firing::Ready(
            self.inputs
                .iter_mut()
                .map(|i| match i.kind {
                    ChannelKind::Queue => i.queue.pop_front().expect("readiness checked"),
                    ChannelKind::Value => i.value.as_ref().expect("readiness checked").clone(),
                })
                .collect(),
        )
    }
}

/// Collect emits one list only on closure and emits nothing for empty input,
/// matching Nextflow's collect operator's default empty-input behavior.
#[derive(Debug, Default)]
pub struct Collect<T> {
    items: Vec<T>,
    closed: bool,
}

impl<T> Collect<T> {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            closed: false,
        }
    }
    pub fn send(&mut self, value: T) -> Result<(), ChannelError> {
        if self.closed {
            return Err(ChannelError::Closed);
        }
        self.items.push(value);
        Ok(())
    }
    pub fn close(&mut self) -> Option<Vec<T>> {
        if self.closed {
            return None;
        }
        self.closed = true;
        if self.items.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.items))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_and_value_match_without_consuming_partial_tuples() {
        let mut ports = InputSet::new([ChannelKind::Queue, ChannelKind::Value]);
        ports.send(0, 1).unwrap();
        ports.send(0, 2).unwrap();
        assert_eq!(ports.next(), Firing::Pending);
        ports.send(1, 9).unwrap();
        ports.close(1).unwrap();
        ports.close(0).unwrap();
        assert_eq!(ports.next(), Firing::Ready(vec![1, 9]));
        assert_eq!(ports.next(), Firing::Ready(vec![2, 9]));
        assert_eq!(ports.next(), Firing::Closed);
    }
    #[test]
    fn multiple_queues_pair_in_arrival_order_until_first_exhausted() {
        let mut ports = InputSet::new([ChannelKind::Queue, ChannelKind::Queue]);
        ports.send(0, 1).unwrap();
        ports.send(0, 2).unwrap();
        ports.send(1, 7).unwrap();
        ports.close(1).unwrap();
        assert_eq!(ports.next(), Firing::Ready(vec![1, 7]));
        assert_eq!(ports.next(), Firing::Closed);
    }
    #[test]
    fn value_only_fires_once_and_unbound_closed_never_fires() {
        let mut ports = InputSet::new([ChannelKind::Value]);
        ports.send(0, 3).unwrap();
        assert_eq!(ports.send(0, 4), Err(ChannelError::AlreadyBound));
        assert_eq!(ports.next(), Firing::Ready(vec![3]));
        assert_eq!(ports.next(), Firing::Closed);
        let mut empty = InputSet::<i32>::new([ChannelKind::Value]);
        empty.close(0).unwrap();
        assert_eq!(empty.next(), Firing::Closed);
    }
    #[test]
    fn collect_waits_for_close_and_empty_emits_nothing() {
        let mut collect = Collect::new();
        collect.send(1).unwrap();
        collect.send(2).unwrap();
        assert_eq!(collect.close(), Some(vec![1, 2]));
        assert_eq!(collect.close(), None);
        assert_eq!(collect.send(3), Err(ChannelError::Closed));
        assert_eq!(Collect::<i32>::new().close(), None);
    }
}
