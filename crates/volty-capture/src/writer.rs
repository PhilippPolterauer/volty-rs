use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use crate::{CaptureError, CaptureFooter, CaptureRecord, SCHEMA};

/// Append-only JSONL writer for a capture file.
///
/// The first record must be a [`CaptureRecord::Header`]. The footer is written
/// by [`CaptureWriter::finish`]; passing a footer to [`CaptureWriter::write`]
/// is rejected.
#[derive(Debug)]
pub struct CaptureWriter {
    writer: BufWriter<File>,
    count: u64,
    header_written: bool,
}

impl CaptureWriter {
    /// Creates or truncates a capture file.
    ///
    /// # Errors
    /// Returns an error when the file cannot be created.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, CaptureError> {
        let path = path.as_ref();
        let file = File::create(path).map_err(|error| {
            CaptureError::new(format!(
                "failed to create capture file {}: {error}",
                path.display()
            ))
        })?;
        Ok(Self {
            writer: BufWriter::new(file),
            count: 0,
            header_written: false,
        })
    }

    /// Writes one record followed by a newline.
    ///
    /// # Errors
    /// Returns an error when the record is not allowed in the current position
    /// (data before a header, a second header, or a footer passed directly) or
    /// when the record cannot be written.
    pub fn write(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        match record {
            CaptureRecord::Header(header) => {
                if self.header_written {
                    return Err(CaptureError::new("capture already has a header"));
                }
                if header.schema != SCHEMA {
                    return Err(CaptureError::new(format!(
                        "capture header schema {:?} does not match {SCHEMA}",
                        header.schema
                    )));
                }
            }
            CaptureRecord::Footer(_) => {
                return Err(CaptureError::new(
                    "capture footer must be written with CaptureWriter::finish",
                ));
            }
            _ if !self.header_written => {
                return Err(CaptureError::new(format!(
                    "capture {} record written before the header",
                    record.kind()
                )));
            }
            _ => {}
        }

        self.write_record(record)?;
        self.count += 1;
        if matches!(record, CaptureRecord::Header(_)) {
            self.header_written = true;
        }
        Ok(())
    }

    /// Writes the footer and flushes the file.
    ///
    /// `complete` records whether the capture was closed deliberately.
    ///
    /// # Errors
    /// Returns an error when no header was written or the footer cannot be
    /// written or flushed.
    pub fn finish(mut self, complete: bool) -> Result<(), CaptureError> {
        if !self.header_written {
            return Err(CaptureError::new(
                "capture cannot be finished before a header is written",
            ));
        }

        let footer = CaptureFooter {
            schema: SCHEMA.to_owned(),
            records: self.count,
            complete,
        };
        self.write_record(&CaptureRecord::Footer(footer))?;
        self.writer
            .flush()
            .map_err(|error| CaptureError::new(format!("failed to flush capture file: {error}")))
    }

    fn write_record(&mut self, record: &CaptureRecord) -> Result<(), CaptureError> {
        serde_json::to_writer(&mut self.writer, record).map_err(|error| {
            CaptureError::new(format!("failed to encode capture record: {error}"))
        })?;
        self.writer
            .write_all(b"\n")
            .map_err(|error| CaptureError::new(format!("failed to write capture record: {error}")))
    }
}
