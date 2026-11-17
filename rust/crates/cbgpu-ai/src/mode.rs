//! Process-local execution policy. No remote executor is implied by API readiness.
use cbgpu_ai_store::{Result, fail};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RuntimeMode {
    #[default]
    Legacy,
    ApiOnly,
    AllInOne,
}
impl RuntimeMode {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value {
            None | Some("") => Ok(Self::Legacy),
            Some("api-only") => Ok(Self::ApiOnly),
            Some("all-in-one") => Ok(Self::AllInOne),
            _ => Err(fail("Invalid native AI runtime mode", 503)),
        }
    }
    pub fn from_environment() -> Result<Self> {
        match std::env::var("CBGPU_AI_MODE") {
            Ok(value) => Self::parse(Some(&value)),
            Err(std::env::VarError::NotPresent) => Ok(Self::Legacy),
            Err(_) => Err(fail("Invalid native AI runtime mode", 503)),
        }
    }
    pub fn attach_runner(self, configured: bool) -> Result<bool> {
        match self {
            Self::ApiOnly => Ok(false),
            Self::AllInOne if !configured => Err(fail(
                "All-in-one mode requires an explicit native runner",
                503,
            )),
            _ => Ok(configured),
        }
    }
    pub fn local_execution(self) -> bool {
        self != Self::ApiOnly
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_api_only_never_attaches_even_with_reused_runner_environment() {
        for present in [false, true] {
            assert!(!RuntimeMode::ApiOnly.attach_runner(present).unwrap());
        }
        assert!(!RuntimeMode::Legacy.attach_runner(false).unwrap());
        assert!(RuntimeMode::Legacy.attach_runner(true).unwrap());
        assert!(RuntimeMode::AllInOne.attach_runner(false).is_err());
        assert!(RuntimeMode::AllInOne.attach_runner(true).unwrap());
        assert!(!RuntimeMode::ApiOnly.local_execution());
        for value in ["API-ONLY", "api_only", " api-only", "all-in-one ", "true"] {
            assert!(RuntimeMode::parse(Some(value)).is_err());
        }
        assert_eq!(RuntimeMode::parse(None).unwrap(), RuntimeMode::Legacy);
        assert_eq!(RuntimeMode::parse(Some("")).unwrap(), RuntimeMode::Legacy);
    }
}
