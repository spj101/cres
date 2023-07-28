mod opt;

use std::cell::RefCell;
use std::rc::Rc;

use crate::opt::{Opt, Search};

use anyhow::{Context, Result};
use clap::Parser;
use cres::converter::ClusteringConverter;
use cres::reader::CombinedReader;
use cres::writer::FileWriter;
use cres::{
    cell_collector::CellCollector,
    distance::{EuclWithScaledPt, PtDistance},
    prelude::*,
    neighbour_search::{NeighbourData, NeighbourSearch, NaiveNeighbourSearch, TreeSearch},
    resampler::DefaultResamplerBuilder, GIT_BRANCH, GIT_REV, VERSION,
};
use env_logger::Env;
use log::{debug, info};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256Plus;
use noisy_float::prelude::*;

fn main() -> Result<()> {
    let args = argfile::expand_args_from(
        std::env::args_os(),
        argfile::parse_fromfile,
        argfile::PREFIX,
    ).with_context(|| "Failed to read argument file")?;
    let opt = Opt::parse_from(args).validate()?;
    match opt.search {
        Search::Naive => run_main::<NaiveNeighbourSearch>(opt),
        Search::Tree => run_main::<TreeSearch>(opt),
    }?;
    info!("done");
    Ok(())
}

fn run_main<N>(opt: Opt) -> Result<()>
where
    N: NeighbourData,
    for <'x, 'y, 'z> &'x mut N: NeighbourSearch<PtDistance<'y, 'z, EuclWithScaledPt>>,
    for <'x, 'y, 'z> <&'x mut N as NeighbourSearch<PtDistance<'y, 'z, EuclWithScaledPt>>>::Iter: Iterator<Item=(usize, N64)>,
{
    let env = Env::default().filter_or("CRES_LOG", &opt.loglevel);
    env_logger::init_from_env(env);

    rayon::ThreadPoolBuilder::new()
        .num_threads(opt.threads)
        .build_global()?;

    if let (Some(rev), Some(branch)) = (GIT_REV, GIT_BRANCH) {
        info!("cres {} rev {} ({})", VERSION, rev, branch);
    } else {
        info!("cres {}", VERSION);
    }

    debug!("settings: {:#?}", opt);

    let reader = CombinedReader::from_files(opt.infiles)?;

    let cell_collector = if opt.dumpcells {
        Some(Rc::new(RefCell::new(CellCollector::new())))
    } else {
        None
    };
    let resampler = DefaultResamplerBuilder::default()
        .max_cell_size(opt.max_cell_size)
        .num_partitions(opt.partitions)
        .ptweight(opt.ptweight)
        .strategy(opt.strategy)
        .cell_collector(cell_collector.clone())
        .neighbour_search::<N>()
        .build();

    let rng = Xoshiro256Plus::seed_from_u64(opt.unweight.seed);

    let unweighter = Unweighter::new(opt.unweight.minweight, rng);
    let mut converter = ClusteringConverter::new(opt.jet_def.into())
        .include_neutrinos(opt.include_neutrinos);
    if opt.lepton_def.leptonalgorithm.is_some() {
        converter = converter.with_lepton_def(opt.lepton_def.into())
    }
    let writer = FileWriter::builder()
        .filename(opt.outfile.clone())
        .format(opt.outformat.into())
        .compression(opt.compression)
        .cell_collector(cell_collector)
        .build();

    let mut cres = CresBuilder {
        reader,
        converter,
        resampler,
        unweighter,
        writer,

    }.build();
    cres.run()?;

    Ok(())
}
