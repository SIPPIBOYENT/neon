use std::fs::{File, OpenOptions};
use std::io;
use std::io::prelude::*;
use std::path::Path;

use anyhow::Result;

use crate::pg_helpers::escape_conf_value;
use crate::pg_helpers::PgOptionsSerialize;
use compute_api::spec::ComputeSpec;

/// Check that `line` is inside a text file and put it there if it is not.
/// Create file if it doesn't exist.
pub fn line_in_file(path: &Path, line: &str) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .append(false)
        .open(path)?;
    let buf = io::BufReader::new(&file);
    let mut count: usize = 0;

    for l in buf.lines() {
        if l? == line {
            return Ok(false);
        }
        count = 1;
    }

    write!(file, "{}{}", "\n".repeat(count), line)?;
    Ok(true)
}

/// Create or completely rewrite configuration file specified by `path`
pub fn write_postgres_conf(path: &Path, spec: &ComputeSpec) -> Result<()> {
    // File::create() destroys the file content if it exists.
    let mut file = File::create(path)?;

    // Write the postgresql.conf content from the spec file as is.
    if let Some(conf) = &spec.cluster.postgresql_conf {
        writeln!(file, "{}", conf)?;
    }

    // Add options for connecting to storage
    writeln!(file, "# Neon storage settings")?;
    if let Some(s) = &spec.pageserver_connstring {
        writeln!(
            file,
            "neon.pageserver_connstring='{}'",
            escape_conf_value(s)
        )?;
    }
    if !spec.safekeeper_connstrings.is_empty() {
        writeln!(
            file,
            "neon.safekeepers='{}'",
            escape_conf_value(&spec.safekeeper_connstrings.join(","))
        )?;
    }
    if let Some(s) = &spec.tenant_id {
        writeln!(
            file,
            "neon.tenant_id='{}'",
            escape_conf_value(&s.to_string())
        )?;
    }
    if let Some(s) = &spec.timeline_id {
        writeln!(
            file,
            "neon.timeline_id='{}'",
            escape_conf_value(&s.to_string())
        )?;
    }

    // If there are any extra options in the 'settings' field, append those
    if spec.cluster.settings.is_some() {
        writeln!(file, "# Managed by compute_ctl: begin")?;
        writeln!(file, "{}", spec.cluster.settings.as_pg_settings())?;
        writeln!(file, "# Managed by compute_ctl: end")?;
    }

    Ok(())
}
