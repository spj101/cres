// TODO: code duplication with hepmc2::Writer
use std::collections::{hash_map::Entry, HashMap};
use std::path::PathBuf;
use std::cell::RefCell;
use std::rc::Rc;

use crate::cell_collector::CellCollector;
use crate::event::Event;
use crate::progress_bar::ProgressBar;
use crate::traits::{Progress, Write};

use derive_builder::Builder;
use thiserror::Error;

/// Write events to ntuple format
#[derive(Debug)]
#[derive(Builder)]
pub struct Writer {
    path: PathBuf,
    #[builder(default)]
    cell_collector: Option<Rc<RefCell<CellCollector>>>,
}

impl<E, R> Write<R> for Writer
where
    R: Iterator<Item = Result<avery::Event, E>>,
    E: std::error::Error,
{
    type Error = WriteError<E>;

    /// Write all `events`.
    fn write(
        &mut self,
        reader: &mut R,
        events: &[Event],
    ) -> Result<(), Self::Error> {
        use WriteError::*;

        let mut writer = ntuple::Writer::new(&self.path, "cres ntuple")
            .ok_or_else(|| CreateWriteErr(self.path.clone()))?;

        let dump_event_to = self
            .cell_collector
            .clone()
            .map(|c| c.borrow().event_cells());
        let mut cell_writers = HashMap::new();
        for cellnr in
            dump_event_to.iter().flat_map(|c| c.values().flatten())
        {
            if let Entry::Vacant(entry) = cell_writers.entry(cellnr) {
                let cell_file = format!("cell{cellnr}.root");
                let ntuplename = format!("cres cell{cellnr}");
                let writer = ntuple::Writer::new(&cell_file, &ntuplename)
                    .ok_or_else(|| CreateWriteErr(cell_file.into()))?;

                entry.insert(writer);
            }
        }

        let mut reader_events = reader.enumerate();
        let progress = ProgressBar::new(events.len() as u64, "events written:");
        for event in events {
            let (reader_id, reader_event) = reader_events.next().unwrap();
            let mut read_event = reader_event.map_err(ReadErr)?;
            if reader_id < event.id() {
                for _ in reader_id..event.id() {
                    let (_id, ev) = reader_events.next().unwrap();
                    read_event = ev.map_err(ReadErr)?;
                }
            }
            // TODO: return error
            let weight = read_event.weights.first_mut().unwrap();
            weight.weight = Some(f64::from(event.weight));
            let out_event = avery::convert(read_event);
            writer.write(&out_event)?;
            if let Some(dump_event_to) = dump_event_to.as_ref() {
                let cellnums: &[usize] = dump_event_to
                    .get(&event.id())
                    .map(|v: &Vec<usize>| v.as_slice())
                    .unwrap_or_default();
                for cellnum in cellnums {
                    let cell_writer = cell_writers.get_mut(cellnum).unwrap();
                    cell_writer.write(&out_event)?;
                }
            }
            progress.inc(1);
        }
        progress.finish();
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum WriteError<E> {
    #[error("Failed to read event: {0}")]
    ReadErr(E),
    #[error("Failed to write event: {0}")]
    WriteErr(#[from] ntuple::writer::WriteError),
    #[error("Create writer to {0}")]
    CreateWriteErr(PathBuf),
}
