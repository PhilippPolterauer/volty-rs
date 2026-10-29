use std::{fs, path::PathBuf};

use crate::{CaptureError, CaptureFooter, CaptureHeader, CaptureRecord, SCHEMA};

/// A validated capture file split into its header, data records, and footer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFile {
    pub header: CaptureHeader,
    /// Data records between the header and footer. Never contains a header or
    /// footer.
    pub records: Vec<CaptureRecord>,
    pub footer: CaptureFooter,
}

impl CaptureFile {
    /// Reads and validates a JSONL capture file.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read or does not satisfy the
    /// capture format rules.
    pub fn read(path: impl AsRef<std::path::Path>) -> Result<Self, CaptureError> {
        CaptureReader::new(path).read()
    }

    /// Checks the header/footer invariants without doing any I/O.
    ///
    /// # Errors
    /// Returns an error when the schema is wrong, the footer record count does
    /// not match, or [`Self::records`] contains a header or footer.
    pub fn validate(&self) -> Result<(), CaptureError> {
        if self.header.schema != SCHEMA {
            return Err(CaptureError::new(format!(
                "capture header schema {:?} does not match {SCHEMA}",
                self.header.schema
            )));
        }
        if self.footer.schema != SCHEMA {
            return Err(CaptureError::new(format!(
                "capture footer schema {:?} does not match {SCHEMA}",
                self.footer.schema
            )));
        }
        if self.footer.schema != self.header.schema {
            return Err(CaptureError::new(format!(
                "capture footer schema {:?} does not match header schema {:?}",
                self.footer.schema, self.header.schema
            )));
        }
        if let Some(record) = self
            .records
            .iter()
            .find(|record| matches!(record, CaptureRecord::Header(_) | CaptureRecord::Footer(_)))
        {
            return Err(CaptureError::new(format!(
                "capture records must not contain a {} record",
                record.kind()
            )));
        }

        let actual = u64::try_from(self.records.len())
            .map_err(|_| CaptureError::new("capture files cannot hold this many records"))?
            .checked_add(1)
            .ok_or_else(|| CaptureError::new("capture file record count overflowed"))?;
        if self.footer.records != actual {
            return Err(CaptureError::new(format!(
                "footer records count {} does not match {actual} records before the footer",
                self.footer.records
            )));
        }
        Ok(())
    }
}

/// Reads a JSONL capture file and validates its structure.
#[derive(Clone, Debug)]
pub struct CaptureReader {
    path: PathBuf,
}

impl CaptureReader {
    pub fn new(path: impl AsRef<std::path::Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Reads, parses, and validates the capture file.
    ///
    /// # Errors
    /// Returns an error with a line number for malformed JSON or an unknown
    /// record type or field. Structural errors identify the missing, duplicate,
    /// or misplaced header or footer.
    pub fn read(&self) -> Result<CaptureFile, CaptureError> {
        let text = fs::read_to_string(&self.path).map_err(|error| {
            CaptureError::new(format!(
                "failed to read capture file {}: {error}",
                self.path.display()
            ))
        })?;
        if text.trim().is_empty() {
            return Err(CaptureError::new("capture file is empty"));
        }

        let mut records = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            if line.trim().is_empty() {
                return Err(CaptureError::new(format!(
                    "line {line_number}: empty capture record"
                )));
            }
            let record = serde_json::from_str::<CaptureRecord>(line).map_err(|error| {
                CaptureError::new(format!(
                    "line {line_number}: invalid capture record: {error}"
                ))
            })?;
            records.push(record);
        }

        into_file(records)
    }
}

fn into_file(records: Vec<CaptureRecord>) -> Result<CaptureFile, CaptureError> {
    let Some(first) = records.first() else {
        return Err(CaptureError::new("capture file is empty"));
    };
    if !matches!(first, CaptureRecord::Header(_)) {
        return Err(CaptureError::new(format!(
            "capture file must start with a header, found {}",
            first.kind()
        )));
    }
    if records
        .iter()
        .filter(|record| matches!(record, CaptureRecord::Header(_)))
        .count()
        > 1
    {
        return Err(CaptureError::new(
            "capture file contains more than one header",
        ));
    }

    let footer_count = records
        .iter()
        .filter(|record| matches!(record, CaptureRecord::Footer(_)))
        .count();
    if footer_count == 0 {
        return Err(CaptureError::new("capture file is missing a footer"));
    }
    if footer_count > 1 {
        return Err(CaptureError::new(
            "capture file contains more than one footer",
        ));
    }

    let last = records
        .last()
        .ok_or_else(|| CaptureError::new("capture file is empty"))?;
    if !matches!(last, CaptureRecord::Footer(_)) {
        return Err(CaptureError::new(format!(
            "capture file must end with a footer, found {}",
            last.kind()
        )));
    }

    let before_footer = u64::try_from(records.len() - 1)
        .map_err(|_| CaptureError::new("capture files cannot hold this many records"))?;
    let header = match &records[0] {
        CaptureRecord::Header(header) => header,
        _ => return Err(CaptureError::new("capture file must start with a header")),
    };
    let footer = match records.last() {
        Some(CaptureRecord::Footer(footer)) => footer,
        _ => return Err(CaptureError::new("capture file must end with a footer")),
    };

    if header.schema != SCHEMA {
        return Err(CaptureError::new(format!(
            "capture header schema {:?} does not match {SCHEMA}",
            header.schema
        )));
    }
    if footer.schema != SCHEMA {
        return Err(CaptureError::new(format!(
            "capture footer schema {:?} does not match {SCHEMA}",
            footer.schema
        )));
    }
    if footer.schema != header.schema {
        return Err(CaptureError::new(format!(
            "capture footer schema {:?} does not match header schema {:?}",
            footer.schema, header.schema
        )));
    }
    if footer.records != before_footer {
        return Err(CaptureError::new(format!(
            "footer records count {} does not match {before_footer} records before the footer",
            footer.records
        )));
    }

    let mut records = records.into_iter();
    let header = match records.next() {
        Some(CaptureRecord::Header(header)) => header,
        _ => return Err(CaptureError::new("capture file must start with a header")),
    };
    let footer = match records.next_back() {
        Some(CaptureRecord::Footer(footer)) => footer,
        _ => return Err(CaptureError::new("capture file must end with a footer")),
    };
    let file = CaptureFile {
        header,
        records: records.collect(),
        footer,
    };
    file.validate()?;
    Ok(file)
}
