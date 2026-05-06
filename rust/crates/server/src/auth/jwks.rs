use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    pub kid: String,
    pub kty: String,
    pub alg: Option<String>,
    pub n: String,
    pub e: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

pub async fn fetch(jwks_url: &str) -> Result<JwkSet, reqwest::Error> {
    reqwest::Client::new()
        .get(jwks_url)
        .send()
        .await?
        .json::<JwkSet>()
        .await
}

pub fn find_key<'a>(set: &'a JwkSet, kid: &str) -> Option<&'a Jwk> {
    set.keys.iter().find(|k| k.kid == kid)
}
