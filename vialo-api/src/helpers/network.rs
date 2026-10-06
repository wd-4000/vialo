use crate::helpers::random;
use crate::http::network::models::NetAuth;

/// New credentials (username, password) for a network's auth type
pub fn generate_credentials(
    auth: Option<&NetAuth>,
    gen_username: Option<i32>,
    gen_password: Option<i32>,
) -> (Option<String>, Option<String>) {
    let Some(auth) = auth else {
        return (None, None);
    };

    let username = (*auth == NetAuth::UsernamePassword)
        .then(|| random::unambiguous_string(gen_username.unwrap_or(8) as usize));
    let password = random::unambiguous_string(gen_password.unwrap_or(16) as usize);

    (username, Some(password))
}
