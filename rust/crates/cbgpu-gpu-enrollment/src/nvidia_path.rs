//! Only administrator-owned NVIDIA tools at known Ubuntu and WSL locations are executable.
const CANDIDATES: [&str; 2] = ["/usr/bin/nvidia-smi", "/usr/lib/wsl/lib/nvidia-smi"];
#[derive(Clone, Copy)]
pub struct Executable {
    pub regular: bool,
    pub ancestors_safe: bool,
    pub uid: u32,
    pub mode: u32,
}
pub fn select(inspect: impl Fn(&str) -> Option<Executable>) -> Option<&'static str> {
    CANDIDATES.into_iter().find(|p| {
        inspect(p).is_some_and(|m| {
            m.regular
                && m.ancestors_safe
                && m.uid == 0
                && m.mode & 0o022 == 0
                && m.mode & 0o111 != 0
        })
    })
}
pub fn trusted() -> Result<&'static str, &'static str> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        select(|p| {
            let canonical = std::fs::canonicalize(p).ok()?;
            let ancestors_safe = [std::path::Path::new(p), canonical.as_path()]
                .into_iter()
                .all(|path| {
                    path.ancestors().skip(1).all(|parent| {
                        std::fs::metadata(parent)
                            .is_ok_and(|m| m.is_dir() && m.uid() == 0 && m.mode() & 0o022 == 0)
                    })
                });
            let m = std::fs::metadata(&canonical).ok()?;
            Some(Executable {
                regular: m.is_file(),
                ancestors_safe,
                uid: m.uid(),
                mode: m.mode(),
            })
        })
        .ok_or("NVIDIA_UNAVAILABLE")
    }
    #[cfg(not(unix))]
    {
        Err("UNSUPPORTED_PLATFORM")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn safe() -> Executable {
        Executable {
            regular: true,
            ancestors_safe: true,
            uid: 0,
            mode: 0o755,
        }
    }
    #[test]
    fn prefers_standard_and_uses_only_reviewed_wsl_fallback() {
        assert_eq!(select(|_| Some(safe())), Some(CANDIDATES[0]));
        assert_eq!(
            select(|p| (p == CANDIDATES[1]).then_some(safe())),
            Some(CANDIDATES[1])
        );
    }
    #[test]
    fn rejects_user_owned_writable_nonexecutable_and_nonregular_files() {
        for bad in [
            Executable {
                ancestors_safe: false,
                ..safe()
            },
            Executable {
                uid: 1000,
                ..safe()
            },
            Executable {
                mode: 0o777,
                ..safe()
            },
            Executable {
                mode: 0o775,
                ..safe()
            },
            Executable {
                mode: 0o644,
                ..safe()
            },
            Executable {
                regular: false,
                ..safe()
            },
        ] {
            assert_eq!(select(|_| Some(bad)), None)
        }
        assert_eq!(select(|_| None), None);
    }
}
