use crate::event::Event;
use crate::distance::Distance;

use noisy_float::prelude::*;
use rayon::prelude::*;
use log::{debug, trace};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Cell<'a> {
    events: &'a mut [(N64, Event)],
    members: Vec<usize>,
    radius: N64,
    weight_sum: N64,
}

impl<'a> Cell<'a> {
    pub fn new<'b: 'a, F: Distance + Sync + Send>(
        events: &'b mut [(N64, Event)],
        seed_idx: usize,
        distance: &F,
        max_size: N64,
    ) -> Self
    {
        let mut weight_sum = events[seed_idx].1.weight;
        debug_assert!(weight_sum < 0.);
        debug!("Cell seed with weight {:e}", weight_sum);
        let mut members = vec![seed_idx];
        let seed = events[seed_idx].1.clone();

        events.par_iter_mut().for_each(
            |(dist, e)| *dist = distance.distance(e, &seed)
        );

        let mut candidates: Vec<_> = (0..events.len()).collect();
        candidates.swap_remove(seed_idx);

        while weight_sum < 0. {
            let nearest = candidates
                .par_iter()
                .enumerate()
                .min_by_key(|(_pos, &idx)| events[idx].0);
            if let Some((pos, &idx)) = nearest {
                candidates.swap_remove(pos);
                trace!(
                    "adding event with distance {}, weight {:e} to cell",
                    events[idx].0,
                    events[idx].1.weight
                );
                if events[idx].0 > max_size { break }
                weight_sum += events[idx].1.weight;
                members.push(idx);
            } else {
                break
            };
        }
        let radius = events[*members.last().unwrap()].0;
        Self{events, members, weight_sum, radius}
    }

    pub fn resample(&mut self) {
        let orig_weight_sum = self.weight_sum();
        if orig_weight_sum == n64(0.) {
            for &idx in &self.members {
                self.events[idx].1.weight = n64(0.);
            }
        } else {
            let mut abs_weight_sum = n64(0.);
            for &idx in &self.members {
                let awt = self.events[idx].1.weight.abs();
                self.events[idx].1.weight = awt;
                abs_weight_sum += awt;
            }
            for &idx in &self.members {
                self.events[idx].1.weight *= orig_weight_sum / abs_weight_sum;
            }
        }
    }

    pub fn nmembers(&self) -> usize {
        self.members.len()
    }

    pub fn nneg_weights(&self) -> usize {
        self.members.iter().filter(|&&idx| self.events[idx].1.weight < 0.).count()
    }

    pub fn radius(&self) -> N64 {
        self.radius
    }

    pub fn weight_sum(&self) -> N64 {
        self.weight_sum
    }

    pub fn iter(&'a self) -> Box<dyn std::iter::Iterator<Item=&'a (N64, Event)> + 'a> {
        Box::new(self.members.iter().map(move |idx| & self.events[*idx]))
    }
}
