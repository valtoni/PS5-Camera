use serde::Serialize;
use thiserror::Error;

pub const EVENT_LOG_SELF_TEST_ARGUMENT: &str = "--event-log-self-test";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceCommand {
    RunScm,
    EventLogSelfTest,
}

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize)]
#[error("unsupported command-line arguments: {arguments:?}")]
pub struct CommandLineError {
    pub arguments: Vec<String>,
}

pub fn parse_service_command<I, S>(arguments: I) -> Result<ServiceCommand, CommandLineError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let arguments = arguments.into_iter().map(Into::into).collect::<Vec<_>>();
    match arguments.as_slice() {
        [] => Ok(ServiceCommand::RunScm),
        [argument] if argument == EVENT_LOG_SELF_TEST_ARGUMENT => {
            Ok(ServiceCommand::EventLogSelfTest)
        }
        _ => Err(CommandLineError { arguments }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_preserve_normal_scm_mode() {
        assert_eq!(
            parse_service_command(std::iter::empty::<String>()),
            Ok(ServiceCommand::RunScm)
        );
    }

    #[test]
    fn self_test_argument_selects_event_log_without_scm() {
        assert_eq!(
            parse_service_command([EVENT_LOG_SELF_TEST_ARGUMENT]),
            Ok(ServiceCommand::EventLogSelfTest)
        );
    }

    #[test]
    fn extra_or_unknown_arguments_are_auditable_errors() {
        let error = parse_service_command([EVENT_LOG_SELF_TEST_ARGUMENT, "extra"]).unwrap_err();
        assert_eq!(
            error.arguments,
            vec![EVENT_LOG_SELF_TEST_ARGUMENT.to_owned(), "extra".to_owned()]
        );
    }
}
