use crate::distance::{Distance, DistWrapper};
use crate::event::Event;
use crate::traits::NeighbourSearch;

use log::{debug, trace};
use noisy_float::prelude::*;

/// A cell
///
/// See [arXiv:2109.07851](https://arxiv.org/abs/2109.07851) for details
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Cell<'a> {
    events: &'a [Event],
    members: Vec<usize>,
    radius: N64,
    weight_sum: N64,
}

impl<'a> Cell<'a> {
    /// Construct a new cell from the given `events` with
    /// `events[seed_idx]` as seed, distance measure `distance` and
    /// neighbour search implementation `neighbour_search`
    pub fn new<'b: 'a, 'c, F: Distance + Sync + Send, N>(
        events: &'b [Event],
        seed_idx: usize,
        distance: &F,
        neighbour_search: N,
    ) -> Self
    where
        for<'x, 'y> N: NeighbourSearch<DistWrapper<'x, 'y, F>>,
        for<'x, 'y> <N as NeighbourSearch<DistWrapper<'x, 'y, F>>>::Iter:
            Iterator<Item = (usize, N64)>,
    {
        let mut weight_sum = events[seed_idx].weight();
        debug_assert!(weight_sum < 0.);
        debug!("Cell seed {seed_idx}  with weight {:e}", weight_sum);
        let mut members = vec![seed_idx];
        let mut radius = n64(0.);

        let neighbours = neighbour_search
            .nearest_in(&seed_idx, DistWrapper::new(distance, events));

        for (next_idx, dist) in neighbours {
            trace!(
                "adding event {next_idx} with distance {dist}, weight {:e} to cell",
                events[next_idx].weight()
            );
            weight_sum += events[next_idx].weight();
            members.push(next_idx);
            radius = dist;
            if weight_sum >= 0. {
                break;
            }
        }
        Self {
            events,
            members,
            weight_sum,
            radius,
        }
    }

    /// Resample
    ///
    /// This redistributes weights in such a way that all weights have
    /// the same sign.
    ///
    /// The current implementation sets all weights to the mean weight
    /// over the cell.
    #[cfg(feature = "multiweight")]
    pub fn resample(&mut self) {
        use std::ops::{Deref, DerefMut};

        fn add_assign(acc: &mut [N64], rhs: &[N64]) {
            use itertools::zip_eq;
            for (lhs, rhs) in zip_eq(acc, rhs) {
                *lhs += rhs;
            }
        }

        self.members.sort_unstable(); // sort to prevent deadlocks
        let mut member_weights = Vec::from_iter(
            self.members.iter().map(|i| self.events[*i].weights.write()),
        );
        let (first, rest) = member_weights.split_first_mut().unwrap();

        let mut avg_wts = std::mem::take(first.deref_mut());
        for idx in rest.iter() {
            add_assign(&mut avg_wts, idx.deref());
        }
        let inv_norm = n64(1. / self.nmembers() as f64);
        for wt in avg_wts.iter_mut() {
            *wt *= inv_norm;
        }
        for idx in rest {
            idx.copy_from_slice(&avg_wts);
        }
        *first.deref_mut() = avg_wts;
    }

    #[cfg(not(feature = "multiweight"))]
    pub fn resample(&mut self) {
        let avg_wt = self.weight_sum() / (self.nmembers() as f64);
        for &idx in &self.members {
            *self.events[idx].weights.write() = avg_wt;
        }
    }

    /// Number of events in cell
    pub fn nmembers(&self) -> usize {
        self.members.len()
    }

    /// Number of negative-weight events in cell
    pub fn nneg_weights(&self) -> usize {
        self.members
            .iter()
            .filter(|&&idx| self.events[idx].weight() < 0.)
            .count()
    }

    /// Cell radius
    ///
    /// This is the largest distance from the seed to any event in the cell.
    pub fn radius(&self) -> N64 {
        self.radius
    }

    /// Sum of central event weights inside the cell
    pub fn weight_sum(&self) -> N64 {
        self.weight_sum
    }

    /// Iterator over cell members
    pub fn iter(&'a self) -> impl std::iter::Iterator<Item = &'a Event> + 'a {
        self.members.iter().map(move |idx| &self.events[*idx])
    }
}
