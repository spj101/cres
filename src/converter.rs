#[cfg(feature = "multiweight")]
use std::collections::{HashMap, HashSet};

use crate::cluster::{
    cluster, is_hadron, is_light_lepton, is_parton, is_photon, JetDefinition,
    PID_DRESSED_LEPTON, PID_JET,
};
use crate::event::{Event, EventBuilder};
use crate::traits::TryConvert;

use avery::event::Status;
use noisy_float::prelude::*;
use particle_id::ParticleID;
use thiserror::Error;

/// Convert an input event into internal format with jet clustering
#[derive(Clone, Debug)]
pub struct ClusteringConverter {
    jet_def: JetDefinition,
    lepton_def: Option<JetDefinition>,
    include_neutrinos: bool,
    #[cfg(feature = "multiweight")]
    weight_names: HashSet<String>,
}

impl ClusteringConverter {
    /// Construct a new converter using the given jet clustering
    pub fn new(jet_def: JetDefinition) -> Self {
        Self {
            jet_def,
            lepton_def: None,
            include_neutrinos: false,
            #[cfg(feature = "multiweight")]
            weight_names: HashSet::new(),
        }
    }

    /// Enable lepton clustering
    pub fn with_lepton_def(mut self, lepton_def: JetDefinition) -> Self {
        self.lepton_def = Some(lepton_def);
        self
    }

    /// Whether to include neutrinos in final event record
    pub fn include_neutrinos(mut self, include: bool) -> Self {
        self.include_neutrinos = include;
        self
    }

    /// Names of additional weights to include in the converted event
    ///
    /// By default, only the main weight is kept
    #[cfg(feature = "multiweight")]
    pub fn include_weights(mut self, weight_names: HashSet<String>) -> Self {
        self.weight_names = weight_names;
        self
    }

    fn is_clustered_to_lepton(&self, id: ParticleID) -> bool {
        self.lepton_def.is_some()
            && (is_light_lepton(id.abs()) || is_photon(id))
    }
}

impl TryConvert<avery::Event, Event> for ClusteringConverter {
    type Error = ConversionError;

    fn try_convert(
        &mut self,
        event: avery::Event,
    ) -> Result<Event, Self::Error> {
        let mut partons = Vec::new();
        let mut leptons = Vec::new();
        let mut builder = EventBuilder::new();
        #[cfg(feature = "multiweight")]
        builder.weights(extract_weights(&event, &self.weight_names)?);
        #[cfg(not(feature = "multiweight"))]
        builder.weights(n64(event.weights.first().unwrap().weight.unwrap()));

        let outgoing = event
            .particles
            .into_iter()
            .filter(|p| p.status == Some(Status::Outgoing));
        for out in outgoing {
            let id = out.id.unwrap();
            let p = out.p.unwrap();
            if is_parton(id) || is_hadron(id.abs()) {
                partons.push(p.into());
            } else if self.is_clustered_to_lepton(id) {
                leptons.push(p.into());
            } else if self.include_neutrinos || !is_neutrino(id) {
                let p = [n64(p[0]), n64(p[1]), n64(p[2]), n64(p[3])];
                builder.add_outgoing(id, p.into());
            }
        }
        let jets = cluster(partons, &self.jet_def);
        for jet in jets {
            let p = [jet.e(), jet.px(), jet.py(), jet.pz()];
            builder.add_outgoing(PID_JET, p.into());
        }
        if let Some(lepton_def) = self.lepton_def.as_ref() {
            let leptons = cluster(leptons, lepton_def);
            for lepton in leptons {
                let p = [lepton.e(), lepton.px(), lepton.py(), lepton.pz()];
                builder.add_outgoing(PID_DRESSED_LEPTON, p.into());
            }
        }
        Ok(builder.build())
    }
}

fn is_neutrino(id: ParticleID) -> bool {
    id.abs().is_neutrino()
}

/// Straightforward conversion into internal format
#[derive(Clone, Default, Debug, Eq, PartialEq)]
pub struct Converter {
    #[cfg(feature = "multiweight")]
    weight_names: HashSet<String>,
}

impl Converter {
    /// Constructor
    pub fn new() -> Self {
        Self::default()
    }

    /// Names of additional weights to include in the converted event
    ///
    /// By default, only the main weight is kept
    #[cfg(feature = "multiweight")]
    pub fn include_weights(mut self, weight_names: HashSet<String>) -> Self {
        self.weight_names = weight_names;
        self
    }
}

impl TryConvert<avery::Event, Event> for Converter {
    type Error = ConversionError;

    fn try_convert(
        &mut self,
        event: avery::Event,
    ) -> Result<Event, Self::Error> {
        let mut builder = EventBuilder::new();
        #[cfg(feature = "multiweight")]
        builder.weights(extract_weights(&event, &self.weight_names)?);
        #[cfg(not(feature = "multiweight"))]
        builder.weights(n64(event.weights.first().unwrap().weight.unwrap()));

        let outgoing = event
            .particles
            .into_iter()
            .filter(|p| p.status == Some(Status::Outgoing));
        for out in outgoing {
            let p = out.p.unwrap();
            let p = [n64(p[0]), n64(p[1]), n64(p[2]), n64(p[3])];
            builder.add_outgoing(out.id.unwrap(), p.into());
        }
        Ok(builder.build())
    }
}

#[cfg(feature = "multiweight")]
fn extract_weights(
    event: &avery::Event,
    weight_names: &HashSet<String>,
) -> Result<Vec<N64>, ConversionError> {
    let mut weights = Vec::with_capacity(weight_names.len() + 1);
    let weight = event.weights.first().unwrap().weight.unwrap();
    weights.push(n64(weight));
    let mut weight_seen: HashMap<_, _> =
        weight_names.iter().map(|n| (n, false)).collect();
    for wt in &event.weights {
        if let Some(name) = wt.name.as_ref() {
            if let Some(seen) = weight_seen.get_mut(name) {
                *seen = true;
                weights.push(n64(wt.weight.unwrap()))
            }
        }
    }
    let missing =
        weight_seen
            .into_iter()
            .find_map(|(name, seen)| if seen { None } else { Some(name) });
    if let Some(missing) = missing {
        let all_names = event
            .weights
            .iter()
            .filter_map(|wt| wt.name.clone())
            .collect();
        Err(ConversionError::WeightNotFound(
            missing.to_owned(),
            all_names,
        ))
    } else {
        Ok(weights)
    }
}

/// Error converting to internal event format
#[derive(Debug, Error)]
pub enum ConversionError {
    /// A named event weight was not found
    #[cfg(feature = "multiweight")]
    #[error("Failed to find event weight \"{0}\": Event has weights {1:?}")]
    WeightNotFound(String, Vec<String>),
}
