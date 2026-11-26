//! Small HTTP helpers shared by the Warcraft Logs and Blizzard clients.
//! Hand-rolled on purpose: no extra crates for base64 or OAuth.

use anyhow::Context;

pub fn b64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().try_proxy_from_env(true).build()
}

/// OAuth client-credentials flow: exchange a client ID and secret for a bearer token.
/// `service` names the API in error messages ("Warcraft Logs", "Battle.net").
pub fn client_credentials(
    agent: &ureq::Agent,
    token_url: &str,
    id: &str,
    secret: &str,
    service: &str,
) -> anyhow::Result<String> {
    let resp = agent
        .post(token_url)
        .set(
            "Authorization",
            &format!("Basic {}", b64(format!("{id}:{secret}").as_bytes())),
        )
        .send_form(&[("grant_type", "client_credentials")]);
    let resp: serde_json::Value = match resp {
        Ok(r) => r.into_json()?,
        Err(ureq::Error::Status(code @ (400 | 401), _)) => {
            anyhow::bail!("{service} rejected the client ID/secret (HTTP {code})")
        }
        Err(e) => return Err(e).with_context(|| format!("requesting {service} access token")),
    };
    Ok(resp["access_token"]
        .as_str()
        .with_context(|| format!("no access_token in the {service} token response"))?
        .to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64() {
        assert_eq!(super::b64(b""), "");
        assert_eq!(super::b64(b"f"), "Zg==");
        assert_eq!(super::b64(b"fo"), "Zm8=");
        assert_eq!(super::b64(b"foo"), "Zm9v");
        assert_eq!(super::b64(b"id:secret"), "aWQ6c2VjcmV0");
    }
}
