mod auto_decompress;
mod hepmc;
mod opt;
mod progress_bar;
mod unweight;

use crate::hepmc::{into_event, CombinedReader};
use crate::opt::Opt;
use crate::progress_bar::{Progress, ProgressBar};
use crate::unweight::unweight;

use std::collections::{hash_map::Entry, HashMap};
use std::fs::File;
use std::io::BufWriter;

use env_logger::Env;
use hepmc2::writer::Writer;
use log::{debug, info, trace};
use noisy_float::prelude::*;
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256Plus;
use rayon::prelude::*;
use structopt::StructOpt;

use cres::cell::Cell;
use cres::distance::distance;
// use cres::parser::parse_event;

fn median_radius(radii: &mut [N64]) -> N64 {
    radii.sort_unstable();
    radii[radii.len() / 2]
}

fn main() {
    if let Err(err) = run_main() {
        eprintln!("ERROR: {}", err)
    }
}

fn run_main() -> Result<(), Box<dyn std::error::Error>> {
    let opt = Opt::from_args();
    let env = Env::default().filter_or("CRES_LOG", &opt.loglevel);
    env_logger::init_from_env(env);

    debug!("settings: {:?}", opt);

    let mut events = Vec::new();

    debug!("Reading events from {:?}", opt.infiles);
    let infiles: Result<Vec<_>, _> =
        opt.infiles.iter().rev().map(File::open).collect();
    let infiles = infiles?;
    let mut reader = CombinedReader::new(infiles);
    for (id, event) in (&mut reader).enumerate() {
        trace!("read event {}", id);
        let mut event = into_event(event?, &opt.jet_def);
        event.id = id;
        events.push(event);
    }

    info!("Read {} events", events.len());

    let orig_sum_wt: N64 = events.iter().map(|e| e.weight).sum();
    let orig_sum_wt2: N64 = events.iter().map(|e| e.weight * e.weight).sum();

    info!(
        "Initial sum of weights: {:e} ± {:e}",
        orig_sum_wt,
        orig_sum_wt2.sqrt()
    );

    let nneg_weight = events.iter().filter(|e| e.weight < 0.).count();
    let progress = ProgressBar::new(nneg_weight as u64, "events treated:");

    let mut cell_radii = Vec::new();
    let mut events: Vec<_> = events.into_par_iter().map(|e| (n64(0.), e)).collect();
    while let Some((n, _)) =
        events.par_iter().enumerate().min_by_key(|(_n, (_dist, e))| e.weight)
    {
        let mut cell_weight = events[n].1.weight;
        if cell_weight >= 0. {
            break;
        }
        debug!("Cell seed with weight {:e}", cell_weight);

        let last_idx = events.len() - 1;
        events.swap(n, last_idx);
        let (mut seed, mut rest) = events.split_last_mut().unwrap();
        seed.0 = n64(0.);
        let seed = seed;

        rest.par_iter_mut().for_each(
            |(dist, e)| *dist = distance(e, &seed.1)
        );

        while cell_weight < 0. {
            let nearest = rest
                .par_iter()
                .enumerate()
                .min_by_key(|(_idx, (dist, _event))| dist);
            let nearest_idx = if let Some((idx, (dist, event))) = nearest {
                trace!(
                    "adding event with distance {}, weight {:e} to cell",
                    dist,
                    event.weight
                );
                cell_weight += event.weight;
                idx
            } else {
                break
            };
            rest.swap(nearest_idx, rest.len() - 1);
            let last_idx = rest.len() - 1;
            rest = &mut rest[..last_idx];
        }
        let rest_len = rest.len();
        let cell = &mut events[rest_len..];
        let mut cell = Cell::with_weight_sum_unchecked(cell, cell_weight);
        progress.inc(cell.nneg_weights() as u64);
        debug!(
            "New cell with {} events, radius {}, and weight {:e}",
            cell.nmembers(),
            cell.radius(),
            cell.weight_sum()
        );
        cell_radii.push(cell.radius());
        cell.resample();
    }
    progress.finish();
    info!("Created {} cells", cell_radii.len());
    info!("Median radius: {}", median_radius(cell_radii.as_mut_slice()));

    let dump_event_to: HashMap<usize, _> = HashMap::new();

    info!("Collecting and sorting events");
    let mut events: Vec<_> = events.into_par_iter().map(|(_dist, event)| event).collect();
    events.par_sort_unstable();

    if opt.unweight.minweight > 0.0 {
        info!("Unweight to minimum weight {:e}", opt.unweight.minweight);
        let mut rng = Xoshiro256Plus::seed_from_u64(opt.unweight.seed);
        unweight(&mut events, opt.unweight.minweight, &mut rng);
    }

    let final_sum_wt: N64 = events.par_iter().map(|e| e.weight).sum();
    let final_sum_wt2: N64 = events.par_iter().map(|e| e.weight * e.weight).sum();

    info!(
        "Final sum of weights: {:e} ± {:e}",
        final_sum_wt,
        final_sum_wt2.sqrt()
    );

    info!("Writing {} events to {:?}", events.len(), opt.outfile);
    reader.rewind()?;
    let outfile = BufWriter::new(File::create(opt.outfile)?);
    let mut cell_writers = HashMap::new();
    for cellnr in dump_event_to.values().flatten() {
        if let Entry::Vacant(entry) = cell_writers.entry(cellnr) {
            let file = File::create(format!("cell{}.hepmc", cellnr))?;
            let writer = Writer::try_from(BufWriter::new(file))?;
            entry.insert(writer);
        }
    }
    let mut writer = Writer::try_from(outfile)?;
    let mut hepmc_events = reader.enumerate();
    let progress = ProgressBar::new(events.len() as u64, "events written:");
    for event in events {
        let (hepmc_id, hepmc_event) = hepmc_events.next().unwrap();
        let mut hepmc_event = hepmc_event.unwrap();
        if hepmc_id < event.id {
            for _ in hepmc_id..event.id {
                let (_, ev) = hepmc_events.next().unwrap();
                ev.unwrap();
            }
            let (id, ev) = hepmc_events.next().unwrap();
            debug_assert_eq!(id, event.id);
            hepmc_event = ev.unwrap();
        }
        let old_weight = hepmc_event.weights.first().unwrap();
        let reweight: f64 = (event.weight / old_weight).into();
        for weight in &mut hepmc_event.weights {
            *weight *= reweight
        }
        writer.write(&hepmc_event)?;
        let cellnums: &[usize] = dump_event_to
            .get(&event.id)
            .map(|v: &Vec<usize>| v.as_slice())
            .unwrap_or_default();
        for cellnum in cellnums {
            let cell_writer = cell_writers.get_mut(cellnum).unwrap();
            cell_writer.write(&hepmc_event)?;
        }
        progress.inc(1);
    }
    writer.finish()?;
    for (_, cell_writer) in cell_writers {
        cell_writer.finish()?;
    }
    progress.finish();
    info!("done");
    Ok(())
}
