use super::models::{NmsError, PpskEntry};

#[allow(async_fn_in_trait)]
pub trait NmsApi {
    async fn set_ppsks(
        &mut self,
        nms_id: &str,
        entries: &[PpskEntry],
    ) -> Result<Vec<NmsError>, anyhow::Error>;
}
