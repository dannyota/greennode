use std::io::Write;

use crate::cli::{query, render};

use super::{Format, LocalError, OperationFailure};

pub enum ExecutionFailure {
    Sdk(OperationFailure),
    Local(LocalError),
}

impl From<OperationFailure> for ExecutionFailure {
    fn from(failure: OperationFailure) -> Self {
        Self::Sdk(failure)
    }
}

impl From<LocalError> for ExecutionFailure {
    fn from(error: LocalError) -> Self {
        Self::Local(error)
    }
}

pub type ExecutionResult =
    Result<Option<Result<render::Projected, render::Error>>, ExecutionFailure>;

pub(crate) type CustomOutput = fn(
    &mut dyn Write,
    &mut dyn Write,
    &render::Projected,
    Format,
    bool,
) -> Result<bool, render::Error>;

#[derive(Clone, Copy, Default)]
pub struct OutputView {
    pub(crate) table: Option<render::TableView>,
    pub(crate) withhold: Option<&'static str>,
    pub(crate) custom: Option<CustomOutput>,
    pub(crate) raw_text: Option<RawText>,
    pub(crate) project_non_json: Option<fn(&mut render::Value)>,
    pub(crate) transform_query_cells: Option<fn(&mut render::Value)>,
    pub(crate) use_for_partial: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct RawText {
    pub(crate) terminal: bool,
    pub(crate) unqueried_field: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RenderFailure {
    Query,
    TableLimit,
    Encoding,
    Output,
}

pub(super) enum OutputError {
    Query,
    Render(render::Error),
}

impl From<render::Error> for OutputError {
    fn from(error: render::Error) -> Self {
        Self::Render(error)
    }
}

pub(super) fn render_output(
    writer: &mut dyn Write,
    stderr: &mut dyn Write,
    projected: &render::Projected,
    format: Format,
    query: Option<&query::CompiledQuery>,
    view: OutputView,
) -> Result<(), OutputError> {
    if let Some(custom) = view.custom
        && custom(writer, stderr, projected, format, query.is_some())?
    {
        return Ok(());
    }
    if view.withhold.is_some() {
        let mut buffer = Vec::new();
        write_output(&mut buffer, projected, format, query, view)?;
        writer.write_all(&buffer).map_err(render::Error::Io)?;
        Ok(())
    } else {
        write_output(writer, projected, format, query, view)
    }
}

fn write_output(
    mut writer: &mut dyn Write,
    projected: &render::Projected,
    format: Format,
    query: Option<&query::CompiledQuery>,
    view: OutputView,
) -> Result<(), OutputError> {
    if format == Format::Json && query.is_none() {
        return render::write_json(&mut writer, projected).map_err(Into::into);
    }
    if let (Format::Table, None, Some(table)) = (format, query, view.table) {
        return render::write_projected_table(&mut writer, projected, table).map_err(Into::into);
    }
    let mut value = projected.for_query()?;
    if format != Format::Json
        && let Some(project) = view.project_non_json
    {
        project(&mut value);
    }
    if let Some(query) = query {
        value = query::evaluate(query, &value).map_err(|_| OutputError::Query)?;
        if format != Format::Json
            && let Some(transform) = view.transform_query_cells
        {
            transform(&mut value);
        }
    }
    if let (Format::Text, Some(raw)) = (format, view.raw_text) {
        let candidate = match (&value, query) {
            (render::Value::Object(fields), None) => fields.get(raw.unqueried_field),
            _ => Some(&value),
        };
        if let Some(render::Value::String(text)) = candidate {
            if raw.terminal {
                for chunk in text.split_inclusive(['\n', '\t']) {
                    let plain = chunk.trim_end_matches(['\n', '\t']);
                    writer
                        .write_all(render::escape(plain).as_bytes())
                        .map_err(render::Error::Io)?;
                    writer
                        .write_all(&chunk.as_bytes()[plain.len()..])
                        .map_err(render::Error::Io)?;
                }
            } else {
                writer
                    .write_all(text.as_bytes())
                    .map_err(render::Error::Io)?;
            }
            return Ok(());
        }
    }
    match format {
        Format::Json => render::write_json(&mut writer, &render::project(&value)?),
        Format::Table => render::write_table(&mut writer, &value),
        Format::Text => render::write_text(&mut writer, &value),
    }
    .map_err(Into::into)
}
