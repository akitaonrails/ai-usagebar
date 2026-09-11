//! Banked "remaining resets" from Grok's consumer billing RPC.
//!
//! Separate from `direct`'s billing call in every way that matters: a
//! different host, a different service, and a protobuf body rather than JSON.
//! The response is `ConsumerGetRemainingResetsResp { repeated ConsumerResetToken
//! tokens = 10 }`, where a token carries `token_id = 10`, `validity_start = 20`
//! and `validity_end = 30`.
//!
//! Only the count and `validity_end` are kept. `token_id` is the handle that
//! *spends* a reset, so it is skipped during parsing rather than parsed and
//! dropped — nothing downstream can leak what was never held.
//!
//! The parser here is deliberately a few dozen lines rather than a protobuf
//! dependency: two message shapes, three fields, one of which is a well-known
//! `Timestamp`. Everything it does not recognise it skips by wire type, so an
//! added field cannot break it.

use std::path::Path;

use chrono::{DateTime, Utc};

use crate::error::{AppError, Result};
use crate::usage::{ResetCredit, ResetCredits};
use crate::vendor::{MAX_BODY_BYTES, read_body_capped, same_origin_redirect_policy};

/// Fixed for the same reason `direct`'s is: this request carries the login's
/// key, so an ambient variable must not choose where the key is sent. Tests
/// reach the seam through [`fetch_with`].
const RESET_URL: &str = "https://grok.com/prod_mc_billing.ConsumerUiSvc/GetRemainingResets";

/// Upper bound on the per-credit rows kept for display. Nobody banks this many
/// resets; the bound exists so a malformed or hostile response cannot turn a
/// tooltip into a six-figure list.
const MAX_LISTED_CREDITS: usize = 64;

pub async fn fetch(auth_path: &Path) -> Result<ResetCredits> {
    fetch_with(auth_path, RESET_URL).await
}

async fn fetch_with(auth_path: &Path, url: &str) -> Result<ResetCredits> {
    let key = super::direct::read_billing_key(auth_path)?;
    let client = reqwest::Client::builder()
        .timeout(crate::vendor::HTTP_CLIENT_TIMEOUT)
        .redirect(same_origin_redirect_policy())
        .build()
        .map_err(|_| AppError::Other("failed to build the Grok reset HTTP client".into()))?;
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/grpc-web+proto")
        .header("x-grpc-web", "1")
        .header("Origin", "https://grok.com")
        .body(vec![0; 5])
        .send()
        .await
        .map_err(|e| AppError::Transport(format!("Grok reset request failed: {e}")))?;
    let status = response.status();
    let bytes = read_body_capped(response, MAX_BODY_BYTES).await?;
    if !status.is_success() {
        return Err(AppError::Http {
            status: status.as_u16(),
            body: String::new(),
        });
    }
    parse_grpc_web(&bytes)
}

/// gRPC-Web frames a body as `[flags: u8][len: u32 BE][payload]`, repeated.
/// A `0x80` flag marks the trailer frame, which is where a gRPC-level failure
/// arrives — with HTTP 200 in front of it. Reading only the data frames would
/// turn "your token was rejected" into an authoritative "0 resets", so the
/// trailer's status is checked before any count is believed.
fn parse_grpc_web(bytes: &[u8]) -> Result<ResetCredits> {
    let mut offset = 0;
    let mut credits = ResetCredits::default();
    while offset < bytes.len() {
        if bytes.len() - offset < 5 {
            return Err(schema_error());
        }
        let flags = bytes[offset];
        let len = u32::from_be_bytes(bytes[offset + 1..offset + 5].try_into().unwrap()) as usize;
        offset += 5;
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(schema_error)?;
        if flags & 0x80 != 0 {
            check_trailer(&bytes[offset..end])?;
        } else if flags == 0 {
            parse_response(&bytes[offset..end], &mut credits)?;
        } else {
            return Err(schema_error());
        }
        offset = end;
    }
    Ok(credits)
}

/// The trailer is HTTP-header text, not protobuf. A missing `grpc-status` is
/// success by the spec's own default; anything else is a failed call.
fn check_trailer(bytes: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(bytes);
    for line in text.split(['\r', '\n']) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("grpc-status") && value.trim() != "0" {
            return Err(AppError::Other(
                "Grok declined the remaining-resets request".into(),
            ));
        }
    }
    Ok(())
}

fn parse_response(bytes: &[u8], credits: &mut ResetCredits) -> Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        let key = read_varint(bytes, &mut offset)?;
        if key == (10 << 3 | 2) {
            let token = read_delimited(bytes, &mut offset)?;
            credits.available = credits.available.checked_add(1).ok_or_else(schema_error)?;
            // Each token becomes a rendered row. The 2 MiB body cap alone
            // still allows a six-figure row count from ~2-byte tokens, so the
            // count keeps rising while the *inventory* stops — the number
            // stays truthful and the tooltip stays a tooltip.
            if credits.credits.len() < MAX_LISTED_CREDITS {
                credits.credits.push(ResetCredit {
                    title: None,
                    expires_at: parse_token(token)?,
                });
            }
        } else {
            skip_field(bytes, &mut offset, key & 7)?;
        }
    }
    Ok(())
}

fn parse_token(bytes: &[u8]) -> Result<Option<DateTime<Utc>>> {
    let mut offset = 0;
    let mut expires_at = None;
    while offset < bytes.len() {
        let key = read_varint(bytes, &mut offset)?;
        if key == (30 << 3 | 2) {
            let timestamp = read_delimited(bytes, &mut offset)?;
            expires_at = parse_timestamp(timestamp)?;
        } else {
            skip_field(bytes, &mut offset, key & 7)?;
        }
    }
    Ok(expires_at)
}

fn parse_timestamp(bytes: &[u8]) -> Result<Option<DateTime<Utc>>> {
    let mut offset = 0;
    let mut seconds = None;
    while offset < bytes.len() {
        let key = read_varint(bytes, &mut offset)?;
        if key == 8 {
            seconds = Some(read_varint(bytes, &mut offset)? as i64);
        } else {
            skip_field(bytes, &mut offset, key & 7)?;
        }
    }
    seconds
        .map(|seconds| DateTime::from_timestamp(seconds, 0).ok_or_else(schema_error))
        .transpose()
}

fn read_delimited<'a>(bytes: &'a [u8], offset: &mut usize) -> Result<&'a [u8]> {
    let len = usize::try_from(read_varint(bytes, offset)?).map_err(|_| schema_error())?;
    let end = offset
        .checked_add(len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(schema_error)?;
    let value = &bytes[*offset..end];
    *offset = end;
    Ok(value)
}

fn read_varint(bytes: &[u8], offset: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *bytes.get(*offset).ok_or_else(schema_error)?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            return Err(schema_error());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(schema_error())
}

fn skip_field(bytes: &[u8], offset: &mut usize, wire_type: u64) -> Result<()> {
    match wire_type {
        0 => {
            read_varint(bytes, offset)?;
        }
        1 => {
            *offset = offset
                .checked_add(8)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(schema_error)?
        }
        2 => {
            read_delimited(bytes, offset)?;
        }
        5 => {
            *offset = offset
                .checked_add(4)
                .filter(|end| *end <= bytes.len())
                .ok_or_else(schema_error)?
        }
        _ => return Err(schema_error()),
    }
    Ok(())
}

fn schema_error() -> AppError {
    AppError::Schema("Grok reset response does not match the expected schema".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn varint(mut value: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn delimited(field: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = varint(field << 3 | 2);
        out.extend(varint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn timestamp(seconds: i64) -> Vec<u8> {
        let mut out = varint(1 << 3);
        out.extend(varint(seconds as u64));
        out
    }

    /// `ConsumerResetToken { token_id = 10, validity_start = 20, validity_end = 30 }`.
    fn token(id: &str, start: i64, end: i64) -> Vec<u8> {
        let mut out = delimited(10, id.as_bytes());
        out.extend(delimited(20, &timestamp(start)));
        out.extend(delimited(30, &timestamp(end)));
        out
    }

    fn data_frame(payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0];
        out.extend((payload.len() as u32).to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn trailer_frame(text: &str) -> Vec<u8> {
        let mut out = vec![0x80];
        out.extend((text.len() as u32).to_be_bytes());
        out.extend_from_slice(text.as_bytes());
        out
    }

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    #[test]
    fn every_remaining_token_is_counted_with_its_own_expiry() {
        let mut body = delimited(10, &token("restok_one", 1_786_560_540, 1_789_238_940));
        body.extend(delimited(
            10,
            &token("restok_two", 1_786_560_540, 1_791_830_940),
        ));
        let parsed = parse_grpc_web(&data_frame(&body)).unwrap();

        assert_eq!(parsed.available, 2);
        assert_eq!(
            parsed
                .credits
                .iter()
                .filter_map(|credit| credit.expires_at)
                .collect::<Vec<_>>(),
            vec![at(1_789_238_940), at(1_791_830_940)]
        );
        assert_eq!(parsed.next_expiry(), Some(at(1_789_238_940)));
    }

    /// `token_id` spends the reset. It must not survive parsing, so that no
    /// later cache write, tooltip, or error message can carry it.
    #[test]
    fn the_redemption_token_id_never_leaves_the_parser() {
        let body = delimited(10, &token("restok_secret", 1, 1_789_238_940));
        let parsed = parse_grpc_web(&data_frame(&body)).unwrap();
        assert!(!format!("{parsed:?}").contains("restok_secret"));
    }

    /// gRPC reports failure with HTTP 200 and a trailer. Treating that as an
    /// empty token list would state "0 resets available" on the strength of a
    /// rejected request.
    #[test]
    fn a_rejected_call_in_a_trailer_is_not_read_as_zero_resets() {
        let mut framed = data_frame(&[]);
        framed.extend(trailer_frame(
            "grpc-status:16\r\ngrpc-message:unauthenticated\r\n",
        ));
        assert!(parse_grpc_web(&framed).is_err());

        let mut ok = data_frame(&delimited(10, &token("restok", 1, 1_789_238_940)));
        ok.extend(trailer_frame("grpc-status:0\r\n"));
        assert_eq!(parse_grpc_web(&ok).unwrap().available, 1);
    }

    #[test]
    fn truncated_or_malformed_frames_are_rejected_rather_than_partially_believed() {
        let body = delimited(10, &token("restok", 1, 1_789_238_940));
        let framed = data_frame(&body);
        for cut in [3, 6, framed.len() - 1] {
            assert!(parse_grpc_web(&framed[..cut]).is_err(), "cut at {cut}");
        }
        // A length prefix that claims more than the frame holds.
        let mut lying = vec![0u8];
        lying.extend(255_u32.to_be_bytes());
        lying.extend_from_slice(&body);
        assert!(parse_grpc_web(&lying).is_err());
        // Wire type 7 does not exist.
        assert!(parse_grpc_web(&data_frame(&[0x0f, 0x00])).is_err());
    }

    #[test]
    fn unknown_fields_are_skipped_instead_of_breaking_the_parse() {
        let mut extended = token("restok", 1, 1_789_238_940);
        extended.extend(delimited(40, b"a field this version has never seen"));
        let mut fixed64 = varint(50 << 3 | 1);
        fixed64.extend([0; 8]);
        extended.extend(fixed64);
        let parsed = parse_grpc_web(&data_frame(&delimited(10, &extended))).unwrap();
        assert_eq!(parsed.available, 1);
        assert_eq!(parsed.next_expiry(), Some(at(1_789_238_940)));
    }

    #[tokio::test]
    async fn sends_an_empty_grpc_web_request_with_bearer_auth() {
        let mut auth = tempfile::NamedTempFile::new().unwrap();
        auth.write_all(br#"{"issuer::client":{"key":"secret-key"}}"#)
            .unwrap();
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/resets")
            .match_header("Authorization", "Bearer secret-key")
            .match_header("Content-Type", "application/grpc-web+proto")
            .match_header("x-grpc-web", "1")
            .match_body(vec![0u8, 0, 0, 0, 0])
            .with_body(data_frame(&delimited(
                10,
                &token("restok", 1, 1_789_238_940),
            )))
            .create_async()
            .await;
        let result = fetch_with(auth.path(), &format!("{}/resets", server.url()))
            .await
            .unwrap();
        mock.assert_async().await;
        assert_eq!(result.available, 1);
        assert_eq!(result.next_expiry(), Some(at(1_789_238_940)));
    }

    #[tokio::test]
    async fn an_http_error_reports_no_credentials_and_no_body() {
        let mut auth = tempfile::NamedTempFile::new().unwrap();
        auth.write_all(br#"{"issuer::client":{"key":"secret-key"}}"#)
            .unwrap();
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/resets")
            .with_status(403)
            .with_body("secret-key is not entitled")
            .create_async()
            .await;
        let error = fetch_with(auth.path(), &format!("{}/resets", server.url()))
            .await
            .unwrap_err()
            .to_string();
        mock.assert_async().await;
        assert!(!error.contains("secret-key"));
    }

    /// The body cap alone permits a six-figure row count from minimal tokens,
    /// and every row is rendered. The count must stay truthful while the
    /// listed inventory stops.
    #[test]
    fn a_flood_of_tokens_is_counted_but_not_all_listed() {
        let flood = MAX_LISTED_CREDITS + 25;
        let mut body = Vec::new();
        for _ in 0..flood {
            body.extend(delimited(
                10,
                &token("restok", 1_786_560_540, 1_789_238_940),
            ));
        }
        let parsed = parse_grpc_web(&data_frame(&body)).expect("a long but valid response parses");

        assert_eq!(
            parsed.available as usize, flood,
            "every token still counts toward the total"
        );
        assert_eq!(
            parsed.credits.len(),
            MAX_LISTED_CREDITS,
            "the per-credit inventory stops at the cap"
        );
    }
}
