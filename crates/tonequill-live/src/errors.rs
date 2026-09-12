use crate::events::{ErrorCode, Failure};

#[derive(Debug, thiserror::Error)]
#[error("{detail}")]
pub struct AppError {
    pub code: ErrorCode,
    pub detail: String,
}
impl AppError {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

pub fn failure(error: &anyhow::Error) -> Failure {
    // Preserve an actual operating-system permission failure even when an
    // outer context identifies the device operation that was attempted.
    if error.chain().any(|cause| {
        cause
            .downcast_ref::<cpal::Error>()
            .is_some_and(|e| e.kind() == cpal::ErrorKind::PermissionDenied)
    }) {
        return Failure {
            code: ErrorCode::PermissionDenied,
            detail: format!("{error:#}"),
        };
    }
    // anyhow's context downcast retains typed context. Iterating std::error's
    // chain alone sees ContextError wrappers and loses the AppError value.
    let code = error
        .downcast_ref::<AppError>()
        .map(|e| e.code)
        .or_else(|| {
            error.chain().find_map(|cause| {
                cause
                    .downcast_ref::<tonequill_core::transfer::TransferError>()
                    .map(|e| {
                        use tonequill_core::transfer::TransferError::*;
                        match e {
                            MissingMetadata => ErrorCode::MissingMetadata,
                            MissingPackets { .. } => ErrorCode::MissingPackets,
                            FileTooLarge => ErrorCode::InvalidArgument,
                            _ => ErrorCode::Integrity,
                        }
                    })
            })
        })
        .or_else(|| {
            error.chain().find_map(|cause| {
                cause
                    .downcast_ref::<tonequill_core::transfer::reliable::ReliableError>()
                    .map(|e| {
                        use tonequill_core::transfer::reliable::ReliableError::*;
                        match e {
                            RetryLimit => ErrorCode::Timeout,
                            Rejected(_) | InvalidControl(_) => ErrorCode::Integrity,
                            _ => ErrorCode::Internal,
                        }
                    })
            })
        })
        .or_else(|| {
            error.chain().find_map(|cause| {
                cause.downcast_ref::<cpal::Error>().map(|e| {
                    if e.kind() == cpal::ErrorKind::PermissionDenied {
                        ErrorCode::PermissionDenied
                    } else {
                        ErrorCode::AudioInterrupted
                    }
                })
            })
        })
        .unwrap_or(ErrorCode::Internal);
    Failure {
        code,
        detail: format!("{error:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_context_survives_anyhow_wrapping() {
        let error = anyhow::anyhow!("input device not found").context(AppError::new(
            ErrorCode::DeviceUnavailable,
            "Microphone unavailable",
        ));
        assert_eq!(failure(&error).code, ErrorCode::DeviceUnavailable);
        let destination =
            anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
                .context(AppError::new(ErrorCode::Destination, "Cannot commit file"));
        assert_eq!(failure(&destination).code, ErrorCode::Destination);
        let permission =
            anyhow::Error::new(cpal::Error::from(cpal::ErrorKind::PermissionDenied)).context(
                AppError::new(ErrorCode::DeviceUnavailable, "Microphone unavailable"),
            );
        assert_eq!(failure(&permission).code, ErrorCode::PermissionDenied);
    }
}
