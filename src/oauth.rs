use crate::{
	client::{CLIENT, OAUTH_CLIENT, OAUTH_IS_ROLLING_OVER, OAUTH_RATELIMIT_REMAINING, OAUTH_READY},
	oauth_resources::ANDROID_APP_VERSION_LIST,
};
use base64::{engine::general_purpose, Engine as _};
use log::{error, info, trace, warn};
use serde_json::json;
use std::{collections::HashMap, sync::atomic::Ordering, sync::Arc, time::Duration};
use tegen::tegen::TextGenerator;
use tokio::time::{error::Elapsed, timeout};

const REDDIT_ANDROID_OAUTH_CLIENT_ID: &str = "ohXpoqrZYub1kg";

const AUTH_ENDPOINT: &str = "https://www.reddit.com";

const OAUTH_TIMEOUT: Duration = Duration::from_secs(15);
const OAUTH_INITIAL_BACKOFF: Duration = Duration::from_secs(10);
const OAUTH_MAX_BACKOFF: Duration = Duration::from_secs(300);
const OAUTH_ATTEMPTS_PER_BACKEND: u32 = 3;

// Response from OAuth backend authentication
#[derive(Debug, Clone)]
pub struct OauthResponse {
	pub token: String,
	pub expires_in: u64,
	pub additional_headers: HashMap<String, String>,
}

// Trait for OAuth backend implementations
trait OauthBackend: Send + Sync {
	fn authenticate(&mut self) -> impl std::future::Future<Output = Result<OauthResponse, AuthError>> + Send;
	fn user_agent(&self) -> &str;
	fn get_headers(&self) -> HashMap<String, String>;
}

// OAuth backend implementations
#[derive(Debug, Clone)]
pub(crate) enum OauthBackendImpl {
	MobileSpoof(MobileSpoofAuth),
	GenericWeb(GenericWebAuth),
}

impl OauthBackend for OauthBackendImpl {
	async fn authenticate(&mut self) -> Result<OauthResponse, AuthError> {
		match self {
			OauthBackendImpl::MobileSpoof(backend) => backend.authenticate().await,
			OauthBackendImpl::GenericWeb(backend) => backend.authenticate().await,
		}
	}

	fn user_agent(&self) -> &str {
		match self {
			OauthBackendImpl::MobileSpoof(backend) => backend.user_agent(),
			OauthBackendImpl::GenericWeb(backend) => backend.user_agent(),
		}
	}

	fn get_headers(&self) -> HashMap<String, String> {
		match self {
			OauthBackendImpl::MobileSpoof(backend) => backend.get_headers(),
			OauthBackendImpl::GenericWeb(backend) => backend.get_headers(),
		}
	}
}

// Spoofed client for Android devices
#[derive(Debug, Clone)]
pub struct Oauth {
	pub(crate) headers_map: HashMap<String, String>,
	expires_in: u64,
	pub(crate) backend: OauthBackendImpl,
}

impl Oauth {
	/// Construct a local-only placeholder so startup never waits on Reddit.
	pub(crate) fn pending() -> Self {
		let backend = OauthBackendImpl::MobileSpoof(MobileSpoofAuth::new());
		Self {
			headers_map: HashMap::new(),
			expires_in: 0,
			backend,
		}
	}

	/// Create a new OAuth client
	pub(crate) async fn new() -> Self {
		// OAuth is required to serve Reddit content, so keep the process alive
		// while the upstream is unavailable. A capped exponential backoff avoids
		// turning an outage into a restart/request storm.
		let mut failure_count = 0_u32;
		let mut backend_failure_count = 0_u32;
		let mut backend = OauthBackendImpl::MobileSpoof(MobileSpoofAuth::new());

		loop {
			let attempt = Self::new_with_timeout_with_backend(backend.clone()).await;
			match attempt {
				Ok(Ok(oauth)) => {
					info!("[✅] Successfully created OAuth client");
					return oauth;
				}
				Ok(Err(e)) => {
					error!("[⛔] Failed to create OAuth client: {} (details: {e:?})", e.summary());
				}
				Err(_) => {
					error!("[⛔] OAuth request exceeded the {OAUTH_TIMEOUT:?} timeout");
				}
			}

			failure_count = failure_count.saturating_add(1);
			backend_failure_count = backend_failure_count.saturating_add(1);

			// Occasionally try the alternative backend, without rapidly hitting both
			// OAuth endpoints during a general connectivity outage.
			if backend_failure_count >= OAUTH_ATTEMPTS_PER_BACKEND {
				backend = match backend {
					OauthBackendImpl::MobileSpoof(_) => {
						warn!("[🔄] Mobile OAuth failed {backend_failure_count} times; trying GenericWeb OAuth next");
						OauthBackendImpl::GenericWeb(GenericWebAuth::new())
					}
					OauthBackendImpl::GenericWeb(_) => {
						warn!("[🔄] GenericWeb OAuth failed {backend_failure_count} times; trying mobile OAuth next");
						OauthBackendImpl::MobileSpoof(MobileSpoofAuth::new())
					}
				};
				backend_failure_count = 0;
			}

			let delay = oauth_retry_delay(failure_count);
			warn!("[⏳] OAuth remains unavailable; retrying in {delay:?} (failure {failure_count})");
			tokio::time::sleep(delay).await;
		}
	}

	async fn new_with_timeout_with_backend(mut backend: OauthBackendImpl) -> Result<Result<Self, AuthError>, Elapsed> {
		timeout(OAUTH_TIMEOUT, async move {
			let response = backend.authenticate().await?;

			// Build headers_map from backend headers + Authorization header
			let mut headers_map = backend.get_headers();
			headers_map.insert("Authorization".to_owned(), format!("Bearer {}", response.token));
			headers_map.extend(response.additional_headers);

			Ok(Self {
				headers_map,
				expires_in: response.expires_in,
				backend,
			})
		})
		.await
	}

	pub fn user_agent(&self) -> &str {
		self.backend.user_agent()
	}
}

/// Start OAuth acquisition in the background. Until the first token arrives,
/// Reddit API requests fail quickly instead of tying up runtime worker threads.
pub fn start_oauth_supervisor() {
	OAUTH_READY.store(false, Ordering::SeqCst);
	tokio::spawn(async {
		let client = Oauth::new().await;
		OAUTH_CLIENT.swap(client.into());
		OAUTH_READY.store(true, Ordering::SeqCst);
		info!("[✅] OAuth is ready; starting refresh supervisor");
		token_daemon().await;
	});
}

#[derive(Debug)]
enum AuthError {
	Wreq(wreq::Error),
	SerdeDeserialize(serde_json::Error),
	Field(&'static str),
}

impl AuthError {
	fn summary(&self) -> String {
		match self {
			Self::Wreq(error) => error.to_string(),
			Self::SerdeDeserialize(error) => error.to_string(),
			Self::Field(error) => (*error).to_owned(),
		}
	}
}

impl From<wreq::Error> for AuthError {
	fn from(err: wreq::Error) -> Self {
		AuthError::Wreq(err)
	}
}

impl From<serde_json::Error> for AuthError {
	fn from(err: serde_json::Error) -> Self {
		AuthError::SerdeDeserialize(err)
	}
}

pub async fn token_daemon() {
	// Monitor for refreshing token
	loop {
		// Get expiry time - be sure to not hold the read lock
		let scheduled_client = OAUTH_CLIENT.load_full();
		let expires_in = scheduled_client.expires_in;

		// Refresh two minutes early without underflowing malformed or unusually
		// short expiry values.
		let duration = token_refresh_delay(expires_in);

		info!("[⏳] Waiting for {duration:?} seconds before refreshing OAuth token...");

		tokio::time::sleep(duration).await;

		// A 401-triggered refresh may have replaced the token while this timer was
		// sleeping. Do not immediately replace that fresh token again.
		if !Arc::ptr_eq(&scheduled_client, &OAUTH_CLIENT.load_full()) {
			info!("[✅] OAuth token was refreshed while the timer slept; rescheduling");
			continue;
		}

		info!("[⌛] {duration:?} Elapsed! Refreshing OAuth token...");

		// Refresh token - in its own scope
		{
			force_refresh_token().await;
		}
	}
}

fn token_refresh_delay(expires_in: u64) -> Duration {
	Duration::from_secs(expires_in.saturating_sub(120).max(1))
}

fn oauth_backoff_base(failure_count: u32) -> Duration {
	let exponent = failure_count.saturating_sub(1).min(5);
	OAUTH_INITIAL_BACKOFF.saturating_mul(1_u32 << exponent).min(OAUTH_MAX_BACKOFF)
}

fn oauth_retry_delay(failure_count: u32) -> Duration {
	let base = oauth_backoff_base(failure_count);
	// Spread retries from multiple instances over an 80%-120% window.
	let jitter_percent = fastrand::u64(80..=120);
	Duration::from_millis((base.as_millis() as u64).saturating_mul(jitter_percent) / 100)
}

struct RefreshGuard;

impl Drop for RefreshGuard {
	fn drop(&mut self) {
		OAUTH_IS_ROLLING_OVER.store(false, Ordering::SeqCst);
	}
}

pub async fn force_refresh_token() {
	if !OAUTH_READY.load(Ordering::SeqCst) {
		trace!("Skipping OAuth refresh while initial authentication is still pending");
		return;
	}
	if OAUTH_IS_ROLLING_OVER.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
		trace!("Skipping refresh token roll over, already in progress");
		return;
	}
	let _refresh_guard = RefreshGuard;

	trace!("Rolling over refresh token. Current rate limit: {}", OAUTH_RATELIMIT_REMAINING.load(Ordering::SeqCst));
	let new_client = Oauth::new().await;
	OAUTH_CLIENT.swap(new_client.into());
	OAUTH_READY.store(true, Ordering::SeqCst);
	OAUTH_RATELIMIT_REMAINING.store(99, Ordering::SeqCst);
}

#[derive(Debug, Clone, Default)]
struct Device {
	oauth_id: String,
	initial_headers: HashMap<String, String>,
	headers: HashMap<String, String>,
	user_agent: String,
}

// MobileSpoofAuth backend - spoofs an Android mobile device
#[derive(Debug, Clone)]
pub struct MobileSpoofAuth {
	device: Device,
	additional_headers: HashMap<String, String>,
}

impl MobileSpoofAuth {
	fn new() -> Self {
		Self {
			device: Device::new(),
			additional_headers: HashMap::new(),
		}
	}
}

impl OauthBackend for MobileSpoofAuth {
	async fn authenticate(&mut self) -> Result<OauthResponse, AuthError> {
		// Construct URL for OAuth token
		let url = format!("{AUTH_ENDPOINT}/auth/v2/oauth/access-token/loid");
		let mut builder = CLIENT.post(&url);

		// Add headers from spoofed client
		for (key, value) in &self.device.initial_headers {
			builder = builder.header(key, value);
		}
		// Set up HTTP Basic Auth - basically just the const OAuth ID's with no password,
		// Base64-encoded. https://en.wikipedia.org/wiki/Basic_access_authentication
		// This could be constant, but I don't think it's worth it. OAuth ID's can change
		// over time and we want to be flexible.
		let auth = general_purpose::STANDARD.encode(format!("{}:", self.device.oauth_id));
		builder = builder.header("Authorization", format!("Basic {auth}"));

		// Set JSON body. I couldn't tell you what this means. But that's what the client sends
		let json = json!({
				"scopes": ["*","email", "pii"]
		});

		trace!("Sending token request to {url}...");

		// Send request
		let resp = builder.json(&json).send().await?;

		trace!("Received response with status {} and length {:?}", resp.status(), resp.headers().get("content-length"));

		// Parse headers - loid header _should_ be saved sent on subsequent token refreshes.
		// Technically it's not needed, but it's easy for Reddit API to check for this.
		// It's some kind of header that uniquely identifies the device.
		// Not worried about the privacy implications, since this is randomly changed
		// and really only as privacy-concerning as the OAuth token itself.
		if let Some(header) = resp.headers().get("x-reddit-loid") {
			let header_val: &wreq::header::HeaderValue = header;
			if let Ok(value) = header_val.to_str() {
				self.additional_headers.insert("x-reddit-loid".to_owned(), value.to_owned());
			}
		}

		// Same with x-reddit-session
		if let Some(header) = resp.headers().get("x-reddit-session") {
			let header_val: &wreq::header::HeaderValue = header;
			if let Ok(value) = header_val.to_str() {
				self.additional_headers.insert("x-reddit-session".to_owned(), value.to_owned());
			}
		}

		trace!("Serializing response...");

		// Serialize response
		let json: serde_json::Value = resp.json().await?;

		trace!("Accessing relevant fields...");

		// Save token and expiry
		let token = json
			.get("access_token")
			.ok_or(AuthError::Field("OAuth response is missing access_token"))?
			.as_str()
			.ok_or(AuthError::Field("OAuth access_token has an invalid type"))?
			.to_string();
		let expires_in = json
			.get("expires_in")
			.ok_or(AuthError::Field("OAuth response is missing expires_in"))?
			.as_u64()
			.ok_or(AuthError::Field("OAuth expires_in has an invalid type"))?;

		info!("[✅] Successfully retrieved OAuth token; expires in {expires_in}s");

		Ok(OauthResponse {
			token,
			expires_in,
			additional_headers: self.additional_headers.clone(),
		})
	}

	fn user_agent(&self) -> &str {
		&self.device.user_agent
	}

	fn get_headers(&self) -> HashMap<String, String> {
		let mut headers = self.device.headers.clone();
		headers.extend(self.additional_headers.clone());
		headers
	}
}

// GenericWebAuth backend - simple web-based authentication
#[derive(Debug, Clone)]
pub struct GenericWebAuth {
	device_id: String,
	user_agent: String,
	additional_headers: HashMap<String, String>,
}

impl GenericWebAuth {
	fn new() -> Self {
		// Generate random 20-character alphanumeric device_id
		let device_id: String = (0..20)
			.map(|_| {
				let idx = fastrand::usize(..62);
				let chars = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
				chars[idx] as char
			})
			.collect();

		info!("[🔄] Using GenericWebAuth");

		Self {
			device_id,
			user_agent: fake_user_agent::get_rua().to_owned(),
			additional_headers: HashMap::new(),
		}
	}
}

impl OauthBackend for GenericWebAuth {
	async fn authenticate(&mut self) -> Result<OauthResponse, AuthError> {
		// Construct URL for OAuth token
		let url = "https://www.reddit.com/api/v1/access_token";
		let mut builder = CLIENT.post(url);

		// Add minimal headers
		builder = builder.header("Host", "www.reddit.com");
		builder = builder.header("User-Agent", &self.user_agent);
		builder = builder.header("Accept", "*/*");
		builder = builder.header("Accept-Language", "en-US,en;q=0.5");
		// builder = builder.header("Accept-Encoding", "gzip, deflate, br, zstd");
		builder = builder.header("Authorization", "Basic M1hmQkpXbGlIdnFBQ25YcmZJWWxMdzo=");
		builder = builder.header("Content-Type", "application/x-www-form-urlencoded");
		builder = builder.header("Sec-GPC", "1");
		builder = builder.header("Connection", "keep-alive");

		// Set up form body
		let body_str = format!("grant_type=https%3A%2F%2Foauth.reddit.com%2Fgrants%2Finstalled_client&device_id={}", self.device_id);

		trace!("Sending GenericWebAuth token request to {url}...");

		// Send request
		let resp: wreq::Response = builder.body(body_str).send().await?;

		trace!("Received response with status {} and length {:?}", resp.status(), resp.headers().get("content-length"));

		// Parse headers - loid header _should_ be saved sent on subsequent token refreshes.
		// Technically it's not needed, but it's easy for Reddit API to check for this.
		// It's some kind of header that uniquely identifies the device.
		// Not worried about the privacy implications, since this is randomly changed
		// and really only as privacy-concerning as the OAuth token itself.
		if let Some(header) = resp.headers().get("x-reddit-loid") {
			let header_val: &wreq::header::HeaderValue = header;
			if let Ok(value) = header_val.to_str() {
				self.additional_headers.insert("x-reddit-loid".to_owned(), value.to_owned());
			}
		}

		// Same with x-reddit-session
		if let Some(header) = resp.headers().get("x-reddit-session") {
			let header_val: &wreq::header::HeaderValue = header;
			if let Ok(value) = header_val.to_str() {
				self.additional_headers.insert("x-reddit-session".to_owned(), value.to_owned());
			}
		}

		trace!("Serializing GenericWebAuth response...");

		// Serialize response
		let json: serde_json::Value = resp.json().await?;

		trace!("Accessing relevant fields...");

		// Parse response - access_token, token_type, device_id, expires_in, scope
		let token = json
			.get("access_token")
			.ok_or(AuthError::Field("OAuth response is missing access_token"))?
			.as_str()
			.ok_or(AuthError::Field("OAuth access_token has an invalid type"))?
			.to_string();
		let expires_in = json
			.get("expires_in")
			.ok_or(AuthError::Field("OAuth response is missing expires_in"))?
			.as_u64()
			.ok_or(AuthError::Field("OAuth expires_in has an invalid type"))?;

		info!("[✅] GenericWebAuth successfully retrieved OAuth token; expires in {expires_in}s");

		// Insert a few necessary headers
		self.additional_headers.insert("Origin".to_owned(), "https://www.reddit.com".to_owned());
		self.additional_headers.insert("User-Agent".to_owned(), self.user_agent.to_owned());

		Ok(OauthResponse {
			token,
			expires_in,
			additional_headers: self.additional_headers.clone(),
		})
	}

	fn user_agent(&self) -> &str {
		&self.user_agent
	}

	fn get_headers(&self) -> HashMap<String, String> {
		self.additional_headers.clone()
	}
}

impl Device {
	fn android() -> Self {
		// Generate uuid
		let uuid = uuid::Uuid::new_v4().to_string();

		// Generate random user-agent
		let android_app_version = choose(ANDROID_APP_VERSION_LIST).to_string();
		let android_version = fastrand::u8(9..=14);

		let android_user_agent = format!("Reddit/{android_app_version}/Android {android_version}");

		let qos = fastrand::u32(1000..=100_000);
		let qos: f32 = qos as f32 / 1000.0;
		let qos = format!("{qos:.3}");

		let codecs = TextGenerator::new().generate("available-codecs=video/avc, video/hevc{, video/x-vnd.on2.vp9|}");

		// Android device headers
		let headers: HashMap<String, String> = HashMap::from([
			("User-Agent".into(), android_user_agent.clone()),
			("x-reddit-retry".into(), "algo=no-retries".into()),
			("x-reddit-compression".into(), "1".into()),
			("x-reddit-qos".into(), qos),
			("x-reddit-media-codecs".into(), codecs),
			("Content-Type".into(), "application/json; charset=UTF-8".into()),
			("client-vendor-id".into(), uuid.clone()),
			("X-Reddit-Device-Id".into(), uuid.clone()),
		]);

		info!("[🔄] Created randomized Android client identity");

		Self {
			oauth_id: REDDIT_ANDROID_OAUTH_CLIENT_ID.to_string(),
			headers: headers.clone(),
			initial_headers: headers,
			user_agent: android_user_agent,
		}
	}
	fn new() -> Self {
		// See https://github.com/redlib-org/redlib/issues/8
		Self::android()
	}
}

fn choose<T: Copy>(list: &[T]) -> T {
	*fastrand::choose_multiple(list.iter(), 1)[0]
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn refresh_delay_never_underflows() {
		assert_eq!(token_refresh_delay(0), Duration::from_secs(1));
		assert_eq!(token_refresh_delay(60), Duration::from_secs(1));
		assert_eq!(token_refresh_delay(3600), Duration::from_secs(3480));
	}

	#[test]
	fn oauth_backoff_is_exponential_and_capped() {
		assert_eq!(oauth_backoff_base(1), Duration::from_secs(10));
		assert_eq!(oauth_backoff_base(2), Duration::from_secs(20));
		assert_eq!(oauth_backoff_base(5), Duration::from_secs(160));
		assert_eq!(oauth_backoff_base(6), Duration::from_secs(300));
		assert_eq!(oauth_backoff_base(100), Duration::from_secs(300));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_mobile_spoof_backend() {
		// Test MobileSpoofAuth backend specifically
		let mut backend = MobileSpoofAuth::new();
		let response = backend.authenticate().await;
		assert!(response.is_ok());
		let response = response.unwrap();
		assert!(!response.token.is_empty());
		assert!(response.expires_in > 0);
		assert!(!backend.user_agent().is_empty());
		assert!(!backend.get_headers().is_empty());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_generic_web_backend() {
		// Test GenericWebAuth backend specifically
		let mut backend = GenericWebAuth::new();
		let response = backend.authenticate().await;
		assert!(response.is_ok());
		let response = response.unwrap();
		assert!(!response.token.is_empty());
		assert!(response.expires_in > 0);
		assert!(!backend.user_agent().is_empty());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_oauth_client() {
		// Integration test - tests the overall Oauth client
		assert!(OAUTH_CLIENT.load_full().headers_map.contains_key("Authorization"));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_oauth_client_refresh() {
		force_refresh_token().await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_oauth_token_exists() {
		let client = OAUTH_CLIENT.load_full();
		let auth_header = client.headers_map.get("Authorization").unwrap();
		assert!(auth_header.starts_with("Bearer "));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_oauth_headers_len() {
		assert!(OAUTH_CLIENT.load_full().headers_map.len() >= 3);
	}

	#[test]
	fn test_creating_device() {
		Device::new();
	}

	#[test]
	fn test_creating_backends() {
		// Test that both backends can be created
		MobileSpoofAuth::new();
		GenericWebAuth::new();
	}
}
