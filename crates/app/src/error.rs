//! Output window errors.

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no displays found")]
    NoMonitors,
    #[error("output display {requested} not found (available: {})", .available.join(", "))]
    MonitorNotFound {
        requested: usize,
        available: Vec<String>,
    },
    #[error("no second display found")]
    NoSecondary,
    #[error("couldn't create the output window: {0}")]
    Window(String),
    #[error("couldn't set up OpenGL 3.3 on the output display: {0}")]
    Gl(String),
    #[error("the output display stopped responding: {0}")]
    Lost(String),
}

impl OutputError {
    /// One plain sentence for the operator, plus what happens next.
    pub fn user_message(&self) -> String {
        match self {
            OutputError::NoMonitors => {
                "No displays were found, so there is nowhere to show slides. Connect a display and try again.".into()
            }
            OutputError::NoSecondary => {
                "Only the operator's display was found, so the output is off. Connect the projector, or choose a display in the toolbar."
                    .into()
            }
            OutputError::MonitorNotFound { requested, available } => format!(
                "Output display {requested} isn't connected ({} display(s) found). The output stays off until you choose a connected display.",
                available.len()
            ),
            OutputError::Window(_) => {
                "The output window couldn't be opened. The output stays off; try turning it on again.".into()
            }
            OutputError::Gl(_) => "This PC's graphics driver can't provide OpenGL 3.3, which the output needs. Update the graphics driver.".into(),
            OutputError::Lost(_) => "The output display stopped responding. Reopening the output window.".into(),
        }
    }
}
