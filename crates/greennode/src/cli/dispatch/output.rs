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
    if let Some(query) = query {
        value = query::evaluate(query, &value).map_err(|_| OutputError::Query)?;
    }
    match format {
        Format::Json => render::write_json(&mut writer, &render::project(&value)?),
        Format::Table => render::write_table(&mut writer, &value),
        Format::Text => render::write_text(&mut writer, &value),
    }
    .map_err(Into::into)
}
