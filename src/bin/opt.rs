use std::fmt::{self, Display};
use std::path::PathBuf;

use cres::cluster::JetAlgorithm;
use cres::compression::Compression;
use cres::seeds::Strategy;

use clap::{Parser, ValueEnum};
use cres::writer::OutputFormat;
use lazy_static::lazy_static;
use regex::Regex;
use strum::{Display, EnumString};
use thiserror::Error;

fn parse_strategy(s: &str) -> Result<Strategy, UnknownStrategy> {
    use Strategy::*;
    match s {
        "Any" | "any" => Ok(Next),
        "MostNegative" | "most_negative" => Ok(MostNegative),
        "LeastNegative" | "least_negative" => Ok(LeastNegative),
        _ => Err(UnknownStrategy(s.to_string())),
    }
}

#[derive(Debug, Clone, Error)]
pub struct UnknownStrategy(pub String);

impl Display for UnknownStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Unknown strategy: {}", self.0)
    }
}

#[derive(Debug, Clone, Error)]
pub(crate) enum ParseCompressionErr {
    #[error("Unknown compression algorithm: {0}")]
    UnknownAlgorithm(String),
    #[error("Level {0} not supported for {1} compression")]
    UnsupportedLevel(String, String),
}

lazy_static! {
    static ref COMPRESSION_RE: Regex =
        Regex::new(r#"^(?P<algo>[[:alpha:]]+)(?P<lvl>_\d+)?$"#).unwrap();
}

const GZIP_DEFAULT_LEVEL: u8 = 6;
const LZ4_DEFAULT_LEVEL: u8 = 0;
const ZSTD_DEFAULT_LEVEL: u8 = 0;

pub(crate) fn parse_compr(s: &str) -> Result<Compression, ParseCompressionErr> {
    use Compression::*;
    use ParseCompressionErr::*;

    let lower_case = s.to_ascii_lowercase();
    let captures = COMPRESSION_RE.captures(&lower_case);
    let captures = if let Some(captures) = captures {
        captures
    } else {
        return Err(UnknownAlgorithm(s.to_owned()));
    };
    let algo = &captures["algo"];
    let lvl_str = &captures.name("lvl");
    match algo {
        "bzip2" | "bz2" => {
            if let Some(lvl_str) = lvl_str {
                Err(UnsupportedLevel(algo.into(), lvl_str.as_str().to_owned()))
            } else {
                Ok(Bzip2)
            }
        }
        "gzip" | "gz" => {
            if let Some(lvl_str) = lvl_str {
                match lvl_str.as_str()[1..].parse::<u8>() {
                    Ok(lvl) if lvl <= 9 => Ok(Gzip(lvl)),
                    _ => Err(UnsupportedLevel(
                        algo.into(),
                        lvl_str.as_str().to_owned(),
                    )),
                }
            } else {
                Ok(Gzip(GZIP_DEFAULT_LEVEL))
            }
        }
        "lz4" => {
            if let Some(lvl_str) = lvl_str {
                match lvl_str.as_str()[1..].parse::<u8>() {
                    Ok(lvl) if lvl <= 16 => Ok(Lz4(lvl)),
                    _ => Err(UnsupportedLevel(
                        algo.into(),
                        lvl_str.as_str().to_owned(),
                    )),
                }
            } else {
                Ok(Lz4(LZ4_DEFAULT_LEVEL))
            }
        }
        "zstd" | "zstandard" => {
            if let Some(lvl_str) = lvl_str {
                match lvl_str.as_str()[1..].parse::<u8>() {
                    Ok(lvl) if lvl <= 19 => Ok(Zstd(lvl)),
                    _ => Err(UnsupportedLevel(
                        algo.into(),
                        lvl_str.as_str().to_owned(),
                    )),
                }
            } else {
                Ok(Zstd(ZSTD_DEFAULT_LEVEL))
            }
        }
        _ => Err(UnknownAlgorithm(s.to_string())),
    }
}

#[derive(Debug, Copy, Clone, Parser)]
pub(crate) struct JetDefinition {
    /// Jet algorithm.
    #[clap(
        short = 'a',
        long,
        help = "Jet algorithm.\nPossible settings are 'anti-kt', 'kt', 'Cambridge-Aachen'."
    )]
    pub jetalgorithm: JetAlgorithm,
    /// Jet radius parameter.
    #[clap(short = 'R', long)]
    pub jetradius: f64,
    #[clap(short = 'p', long)]
    /// Minimum jet transverse momentum in GeV.
    pub jetpt: f64,
}

impl std::convert::From<JetDefinition> for cres::cluster::JetDefinition {
    fn from(j: JetDefinition) -> Self {
        Self {
            algorithm: j.jetalgorithm,
            radius: j.jetradius,
            min_pt: j.jetpt,
        }
    }
}

#[derive(Debug, Copy, Clone, Parser)]
pub(crate) struct LeptonDefinition {
    /// Lepton dressing algorithm.
    #[clap(
        long,
        help = "Lepton dressing algorithm.\nPossible settings are 'anti-kt', 'kt', 'Cambridge-Aachen'."
    )]
    pub leptonalgorithm: Option<JetAlgorithm>,
    /// Lepton radius parameter.
    #[clap(long)]
    pub leptonradius: Option<f64>,
    #[clap(long)]
    /// Minimum lepton transverse momentum in GeV.
    pub leptonpt: Option<f64>,
}

impl std::convert::From<LeptonDefinition> for cres::cluster::JetDefinition {
    fn from(l: LeptonDefinition) -> Self {
        Self {
            algorithm: l.leptonalgorithm.unwrap(),
            radius: l.leptonradius.unwrap(),
            min_pt: l.leptonpt.unwrap(),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub(crate) enum Search {
    #[default]
    Tree,
    Naive,
}

#[derive(Debug, Default, Copy, Clone, Parser)]
pub(crate) struct UnweightOpt {
    /// Weight below which events are unweighted. '0' means no unweighting.
    #[clap(short = 'w', long, default_value = "0.")]
    pub(crate) minweight: f64,

    /// Random number generator seed for unweighting.
    #[clap(long, default_value = "0")]
    pub(crate) seed: u64,
}

#[derive(Debug, Display, Default, Copy, Clone, ValueEnum, EnumString)]
#[clap(rename_all = "lower")]
pub(crate) enum FileFormat {
    #[default]
    HepMC2,
    #[cfg(feature = "lhef")]
    Lhef,
    #[cfg(feature = "ntuple")]
    Root,
    #[cfg(feature = "stripper-xml")]
    #[clap(name = "stripper-xml")]
    StripperXml,
}

impl From<FileFormat> for OutputFormat {
    fn from(source: FileFormat) -> Self {
        match source {
            FileFormat::HepMC2 => OutputFormat::HepMC2,
            #[cfg(feature = "lhef")]
            FileFormat::Lhef => OutputFormat::Lhef,
            #[cfg(feature = "ntuple")]
            FileFormat::Root => OutputFormat::Root,
            #[cfg(feature = "stripper-xml")]
            FileFormat::StripperXml => OutputFormat::StripperXml,
        }
    }
}

#[derive(Debug, Parser)]
#[clap(about, author, version)]
pub(crate) struct Opt {
    /// Output file.
    #[clap(long, short, value_parser)]
    pub(crate) outfile: PathBuf,

    #[clap(flatten)]
    pub(crate) jet_def: JetDefinition,

    #[clap(flatten)]
    pub(crate) lepton_def: LeptonDefinition,

    /// Include neutrinos in the distance measure
    #[clap(long, default_value_t)]
    pub(crate) include_neutrinos: bool,

    #[clap(flatten)]
    pub(crate) unweight: UnweightOpt,

    /// Weight of transverse momentum when calculating particle momentum distances.
    #[clap(long, default_value = "0.")]
    pub(crate) ptweight: f64,

    /// Whether to dump selected cells of interest.
    #[clap(short = 'd', long)]
    pub(crate) dumpcells: bool,

    #[clap(long, value_parser = parse_compr,
                help = "Compress output file.
Possible settings are 'bzip2', 'gzip', 'zstd', 'lz4'.
Compression levels can be set with algorithm_level e.g. 'zstd_5'.
Maximum levels are 'gzip_9', 'zstd_19', 'lz4_16'.")]
    pub(crate) compression: Option<Compression>,

    /// Output format.
    #[clap(value_enum, long, default_value_t)]
    pub(crate) outformat: FileFormat,

    /// Verbosity level
    #[clap(
        short,
        long,
        default_value = "Info",
        help = "Verbosity level.
Possible values with increasing amount of output are
'off', 'error', 'warn', 'info', 'debug', 'trace'.\n"
    )]
    pub(crate) loglevel: String,

    /// Algorithm for finding nearest-neighbour events.
    #[clap(value_enum, short, long, default_value = "tree")]
    pub(crate) search: Search,

    #[clap(
        long, default_value = "most_negative",
        value_parser = parse_strategy,
        help = "Strategy for choosing cell seeds. Possible values are
'least_negative': event with negative weight closest to zero,
'most_negative' event with the lowest weight,
'any': no additional requirements beyond a negative weight.\n"
    )]
    pub(crate) strategy: Strategy,

    #[clap(
        short,
        long,
        default_value_t,
        help = "Number of threads.

If set to 0, a default number of threads is chosen.
The default can be set with the `RAYON_NUM_THREADS` environment
variable."
    )]
    pub(crate) threads: usize,

    /// Maximum cell size in GeV.
    ///
    /// Limiting the cell size ensures that event weights are only
    /// redistributed between events that are sufficiently similar.
    /// The downside is that not all negative weights may be cancelled.
    #[clap(long)]
    pub(crate) max_cell_size: Option<f64>,

    /// Comma-separated list of weights to include in the resampling
    ///
    /// In addition to the main event weight, weights with the given
    /// names will be averaged within each cell.
    // Would be nice to use a HashSet here, but clap refuses to parse
    // that out of the box
    #[cfg(feature = "multiweight")]
    #[clap(long, value_delimiter = ',')]
    pub(crate) weights: Vec<String>,

    /// Input files
    #[clap(name = "INFILES", value_parser)]
    pub(crate) infiles: Vec<PathBuf>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Error)]
pub(crate) enum ValidationError {
    #[error("Either all or none of --leptonalgorithm, --leptonradius, --leptonpt have to be set")]
    BadLeptonOpt,
}

impl Opt {
    pub(crate) fn validate(self) -> Result<Self, ValidationError> {
        let &LeptonDefinition {
            leptonalgorithm,
            leptonpt,
            leptonradius,
        } = &self.lepton_def;
        match (leptonalgorithm, leptonpt, leptonradius) {
            (Some(_), Some(_), Some(_)) => Ok(self),
            (None, None, None) => Ok(self),
            _ => Err(ValidationError::BadLeptonOpt),
        }
    }
}
