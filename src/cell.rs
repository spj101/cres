use crate::event::Event;

use noisy_float::prelude::*;

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Default)]
pub struct Cell {
    events: Vec<Event>,
    radius: N64,
    weight_sum: N64,
}

impl Cell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_seed(seed: Event) -> Self {
        let weight_sum = seed.weight;
        Self {
            events: vec![seed],
            radius: n64(0.),
            weight_sum,
        }
    }

    pub fn push(&mut self, event: Event) {
        self.weight_sum += event.weight;
        self.events.push(event);
    }

    pub fn push_with_dist(&mut self, event: Event, distance: N64) {
        self.push(event);
        self.radius = distance;
    }

    pub fn nmembers(&self) -> usize {
        self.events.len()
    }

    pub fn radius(&self) -> N64 {
        self.radius
    }

    pub fn weight_sum(&self) -> N64 {
        self.weight_sum
    }
}

impl std::convert::From<Cell> for Vec<Event> {
    fn from(cell: Cell) -> Self {
        cell.events
    }
}
