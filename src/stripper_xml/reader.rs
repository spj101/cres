use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::{Debug, Display},
    io::{BufRead, BufReader, Error, ErrorKind, Read, Seek},
    num::ParseIntError,
    path::{Path, PathBuf},
    str::Utf8Error,
};

use audec::auto_decompress;
use log::debug;
use quick_xml::DeError;
use stripper_xml::{normalization::Normalization, SubEvent};
use thiserror::Error;

use crate::{
    file::File,
    reader::{CreateError, EventFileReader, EventReadError, RewindError},
    traits::{Rewind, TryClone},
    util::trim_ascii_start,
};

/// Read events in STRIPPER XML format from a (potentially compressed) file
pub struct FileReader {
    reader: Reader<Box<dyn BufRead>>,
    source: File,
}

impl FileReader {
    /// Constructor
    ///
    /// Construct a reader for the given (potentially compressed)
    /// STRIPPER XML file with the given information for
    /// channel-specific scale factors
    pub fn new(
        source: File,
        scaling: &HashMap<String, f64>,
    ) -> Result<Self, std::io::Error> {
        let cloned_source = source.try_clone()?;
        let input = auto_decompress(BufReader::new(cloned_source));
        let reader = Reader::with_scaling(input, scaling)
            .map_err(|err| create_error(&source, err))?;
        Ok(FileReader { source, reader })
    }
}

impl Rewind for FileReader {
    type Error = RewindError;

    fn rewind(&mut self) -> Result<(), Self::Error> {
        use RewindError::*;
        self.source.rewind()?;
        let cloned_source = self.source.try_clone().map_err(CloneError)?;
        let input = auto_decompress(BufReader::new(cloned_source));
        let scale = self.reader.scale();
        self.reader = Reader::new_scaled(input, scale)
            .map_err(|err| create_error(&self.source, err))?;

        Ok(())
    }
}

impl EventFileReader for FileReader {}

impl Iterator for FileReader {
    type Item = Result<avery::Event, EventReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        let name = self.reader.name().to_owned();
        self.reader.next().map(|r| match r {
            Ok(ev) => {
                let mut ev = avery::Event::from(ev);
                ev.info = name;
                ev.attr.insert(
                    "wtscale".to_owned(),
                    self.reader.scale().to_string(),
                );
                ev.attr.insert(
                    "as".to_owned(),
                    self.reader.alpha_s_power().to_string(),
                );
                Ok(ev)
            }
            Err(err) => Err(err.into()),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.reader.size_hint()
    }
}

fn create_error(file: impl Debug, err: impl Display) -> Error {
    Error::new(
        ErrorKind::Other,
        format!("Failed to create STRIPPER XML reader for {file:?}: {err}"),
    )
}

struct Reader<T> {
    name: String,
    source: quick_xml::Reader<T>,
    scale: f64,
    alpha_s_power: u64,
    rem_subevents: usize,
    buf: Vec<u8>,
}

impl<T: BufRead> Reader<T> {
    fn new_scaled(mut source: T, scale: f64) -> Result<Self, XMLError> {
        match extract_xml_info(&mut source)? {
            XMLTag::Normalization { .. } => {
                Err(XMLError::BadTag("Normalization".to_owned()))
            }
            XMLTag::Eventrecord {
                alpha_s_power,
                name,
                nevents: _,
                nsubevents,
            } => {
                let rem_subevents = nsubevents as usize;
                Ok(Self {
                    alpha_s_power,
                    name,
                    source: quick_xml::Reader::from_reader(source),
                    scale,
                    rem_subevents,
                    buf: Vec::new(),
                })
            }
        }
    }

    fn with_scaling(
        mut source: T,
        scaling: &HashMap<String, f64>,
    ) -> Result<Self, XMLError> {
        match extract_xml_info(&mut source)? {
            XMLTag::Normalization { .. } => {
                Err(XMLError::BadTag("Normalization".to_owned()))
            }
            XMLTag::Eventrecord {
                alpha_s_power,
                name,
                nevents: _,
                nsubevents,
            } => {
                let rem_subevents = nsubevents as usize;
                let scale = scaling.get(&name).copied().unwrap_or(1.);
                Ok(Self {
                    alpha_s_power,
                    name,
                    source: quick_xml::Reader::from_reader(source),
                    scale,
                    rem_subevents,
                    buf: Vec::new(),
                })
            }
        }
    }

    fn scale(&self) -> f64 {
        self.scale
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn alpha_s_power(&self) -> u64 {
        self.alpha_s_power
    }
}

impl<T: BufRead> Iterator for Reader<T> {
    type Item = Result<SubEvent, ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        use quick_xml::events::Event;
        use ReadError::*;

        self.rem_subevents = self.rem_subevents.saturating_sub(1);
        loop {
            self.buf.clear();
            let read = match self.source.read_event_into(&mut self.buf) {
                Ok(read) => read,
                Err(err) => {
                    use quick_xml::Error;
                    match err {
                        Error::EndEventMismatch {
                            expected: _expected,
                            found,
                        } if found == "Eventrecord" => continue,
                        err => return Some(Err(ParseError(err))),
                    }
                }
            };
            match read {
                Event::Start(tag) => match tag.name().as_ref() {
                    b"e" => {}
                    b"se" => {
                        // restore tag delimiters
                        self.buf.insert(0, b'<');
                        self.buf.push(b'>');

                        // TODO: this is a bad hack, but the following
                        // code alone consumes too many bytes
                        // ```
                        // let mut de = Deserializer::from_reader(rest);
                        // SubEvent::deserialize(&mut de)
                        // ```
                        let mut reader = self.source.get_mut();
                        let read = read_into_until(
                            &mut reader,
                            &mut self.buf,
                            b"</se>",
                        );
                        self.buf.extend_from_slice(b"</se>");
                        if let Err(err) = read {
                            return Some(Err(err.into()));
                        }
                        use quick_xml::de::Deserializer;
                        use serde::de::Deserialize;
                        let mut de =
                            Deserializer::from_reader(self.buf.as_slice());
                        let mut ev = match SubEvent::deserialize(&mut de) {
                            Ok(ev) => ev,
                            Err(err) => return Some(Err(err.into())),
                        };
                        ev.weight *= self.scale();
                        return Some(Ok(ev));
                    }
                    tag => {
                        let tag = match std::str::from_utf8(tag) {
                            Ok(tag) => tag,
                            Err(err) => return Some(Err(err.into())),
                        };
                        return Some(Err(BadTag(tag.to_owned())));
                    }
                },
                Event::End(tag) => match tag.name().as_ref() {
                    b"e" | b"se" | b"Eventrecord" => {}
                    tag => {
                        let tag = match std::str::from_utf8(tag) {
                            Ok(tag) => tag,
                            Err(err) => return Some(Err(err.into())),
                        };
                        return Some(Err(BadTag(tag.to_owned())));
                    }
                },
                Event::Eof => return None,
                Event::Comment(_) | Event::DocType(_) => {}
                Event::Text(t) => {
                    if !trim_ascii_start(t.as_ref()).is_empty() {
                        return Some(Err(BadEntry(format!("{t:?}"))));
                    }
                }
                _ => return Some(Err(BadEntry(format!("{read:?}")))),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.rem_subevents, Some(self.rem_subevents))
    }
}

fn read_into_until<T: BufRead>(
    mut reader: T,
    buf: &mut Vec<u8>,
    until: &[u8],
) -> std::io::Result<()> {
    loop {
        let read = reader.fill_buf()?;
        if let Some(pos) = memchr::memmem::find(read, until) {
            buf.extend_from_slice(&read[..pos]);
            reader.consume(pos);
            return Ok(());
        }
        buf.extend_from_slice(read);
        let len = read.len();
        reader.consume(len);
    }
}

/// Error reading a STRIPPER XML event
#[derive(Debug, Error)]
pub enum ReadError {
    /// XML parsing error
    #[error("Parsing error")]
    ParseError(#[from] quick_xml::Error),
    /// Unexpected XML tag
    #[error("Unexpected XML tag: {0}")]
    BadTag(String),
    /// Unexpected XML entry
    #[error("Unexpected XML entry: {0}")]
    BadEntry(String),
    /// Deserilialisation error
    #[error("Error deserialising event")]
    BadEvent(#[from] DeError),
    /// UTF8 error
    #[error("Utf8 error")]
    Utf8(#[from] Utf8Error),
    /// I/O error
    #[error("I/O error")]
    Io(#[from] std::io::Error),
}

pub(crate) fn extract_scaling<I, P>(
    paths: I,
) -> Result<(Vec<PathBuf>, HashMap<String, f64>), CreateError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut event_files = Vec::new();
    let mut rescale: HashMap<_, (f64, u64)> = HashMap::new();
    for path in paths {
        let path = path.as_ref();
        let file = File::open(path)?;
        let mut r = auto_decompress(BufReader::new(file));
        if let Ok(buf) = r.fill_buf() {
            let buf = trim_ascii_start(buf);
            if buf.starts_with(b"<?xml") {
                debug!("extracting scaling information from {path:?}");
                let tag = extract_xml_info(r).map_err(|err| {
                    crate::reader::CreateError::XMLError(path.to_owned(), err)
                })?;
                match tag {
                    XMLTag::Normalization { name, scale } => {
                        let entry = rescale.entry(name).or_default();
                        entry.0 = scale;
                        // don't add to vec of event files
                    }
                    XMLTag::Eventrecord { name, nevents, .. } => {
                        let entry = rescale.entry(name).or_insert((-1., 0));
                        entry.1 += nevents;
                        event_files.push(path.to_owned())
                    }
                }
            } else {
                // not a STRIPPER XML file
                event_files.push(path.to_owned());
            }
        } else {
            event_files.push(path.to_owned())
        }
    }
    let rescale = rescale
        .into_iter()
        .map(|(name, (scale, nevents))| (name, scale / (nevents as f64)))
        .collect();
    Ok((event_files, rescale))
}

pub(crate) fn extract_xml_info(r: impl BufRead) -> Result<XMLTag, XMLError> {
    use quick_xml::events::Event;
    use XMLError::*;

    let mut buf = Vec::new();
    let mut reader = quick_xml::Reader::from_reader(r);
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match e.name().as_ref() {
                    b"Normalization" => {
                        // restore tag delimiters
                        buf.insert(0, b'<');
                        buf.push(b'>');
                        let rest = reader.into_inner();
                        let all = buf.chain(rest);
                        let norm: Normalization =
                            quick_xml::de::from_reader(all)?;
                        return Ok(XMLTag::Normalization {
                            name: norm.contribution.name,
                            scale: norm.contribution.xsection.0[0],
                        });
                    }
                    b"Eventrecord" => {
                        let mut name = None;
                        let mut nevents = None;
                        let mut nsubevents = None;
                        let mut alpha_s_power = None;
                        let attributes =
                            e.attributes().filter_map(|a| match a {
                                Ok(a) => Some(a),
                                Err(_) => None,
                            });
                        for attr in attributes {
                            match attr.key.0 {
                                b"nevents" => {
                                    nevents =
                                        Some(parse_u64(attr.value.as_ref())?)
                                }
                                b"nsubevents" => {
                                    nsubevents =
                                        Some(parse_u64(attr.value.as_ref())?)
                                }
                                b"name" => name = Some(to_string(attr.value)?),
                                b"as" => {
                                    alpha_s_power =
                                        Some(parse_u64(attr.value.as_ref())?)
                                }
                                _ => {}
                            }
                        }
                        let Some(name) = name else {
                            return Err(NoEventrecordAttr("name"));
                        };
                        let Some(nsubevents) = nsubevents else {
                            return Err(NoEventrecordAttr("nsubevents"));
                        };
                        let Some(nevents) = nevents else {
                            return Err(NoEventrecordAttr("nevents"));
                        };
                        let Some(alpha_s_power) = alpha_s_power else {
                            return Err(NoEventrecordAttr("as"));
                        };
                        return Ok(XMLTag::Eventrecord {
                            alpha_s_power,
                            name,
                            nevents,
                            nsubevents,
                        });
                    }
                    name => {
                        let name = std::str::from_utf8(name)?;
                        return Err(BadTag(name.to_owned()));
                    }
                }
            }
            Ok(Event::Decl(_) | Event::Text(_)) => {} // ignore,
            _ => return Err(NoTag),
        }
    }
}

fn to_string(value: Cow<[u8]>) -> Result<String, XMLError> {
    match value {
        Cow::Borrowed(s) => Ok(std::str::from_utf8(s)?.to_owned()),
        Cow::Owned(s) => Ok(String::from_utf8(s).map_err(|e| e.utf8_error())?),
    }
}

fn parse_u64(num: &[u8]) -> Result<u64, XMLError> {
    let num: &str = std::str::from_utf8(num)?;
    let num = num.parse()?;
    Ok(num)
}

#[derive(Clone, Debug, PartialEq, PartialOrd)]
pub(crate) enum XMLTag {
    Normalization {
        name: String,
        scale: f64,
    },
    Eventrecord {
        alpha_s_power: u64,
        name: String,
        nevents: u64,
        nsubevents: u64,
    },
}

/// STRIPPER XML Error
#[derive(Debug, Error)]
pub enum XMLError {
    /// Error opening a file
    #[error("Failed to open file: {0}")]
    FileOpen(#[from] std::io::Error),
    /// Missing XML tag
    #[error("File does not start with an XML tag")]
    NoTag,
    /// Unsupported XML tag
    #[error("File starts with an unsupported XML tag `{0}`")]
    BadTag(String),
    /// Missing attribute in event record
    #[error("XML tag `Eventrecord` does not have a `{0}` attribute")]
    NoEventrecordAttr(&'static str),
    /// Deserialisation error for [stripper_xml::Normalization]
    #[error("Failed to deserialise `Normalization`: {0}")]
    NormalizationDeser(#[from] quick_xml::DeError),
    /// UTF8 error
    #[error("UTF8 error: {0}")]
    Utf8(#[from] Utf8Error),
    /// Error parsing an integer
    #[error("Failed to parse integer: {0}")]
    ParseInt(#[from] ParseIntError),
}
