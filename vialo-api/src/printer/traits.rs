#[allow(async_fn_in_trait)]
pub trait PrinterApi {
    async fn login(&mut self) -> Result<(), anyhow::Error>;
    async fn lock(&mut self) -> Result<String, anyhow::Error>;
    async fn unlock(&mut self) -> Result<(), anyhow::Error>;
    async fn get_counter_list(&mut self) -> Result<(Vec<i32>, Vec<i32>, Vec<i32>), anyhow::Error>;
    async fn get_username_list(&mut self) -> Result<(Vec<i32>, Vec<String>), anyhow::Error>;
    async fn clear_counter(&mut self, account: u16) -> Result<(), anyhow::Error>;
    async fn get_username(&mut self, account: u16) -> Result<String, anyhow::Error>;
    async fn delete_user(&mut self, account: u16) -> Result<u16, anyhow::Error>;
    /// Set the per-product page limits of an account. `None` means no limit at
    /// all, `Some(0)` disables the product for that account.
    async fn set_user_limit(
        &mut self,
        account: u16,
        username: String,
        color_limit: Option<u16>,
        bw_limit: Option<u16>,
    ) -> Result<u16, anyhow::Error>;
    async fn create_user(
        &mut self,
        address: String,
        username: String,
        password: String,
    ) -> Result<u16, anyhow::Error>;
    /// Change an account's password in place, keeping its slot and counters.
    async fn set_user_password(
        &mut self,
        account: u16,
        username: String,
        password: String,
    ) -> Result<(), anyhow::Error>;
    fn printer_lock(&self) -> &Option<String>;
}
