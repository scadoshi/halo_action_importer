# API Clients

## Overview
Three API clients handle communication with Halo instance:
1. **AuthClient**: OAuth2 authentication and token management
2. **ActionClient**: Posting actions to Halo
3. **ReportClient**: Fetching existing action IDs from reports

---

## AuthClient

**Location**: `src/lib/outbound/client/auth/mod.rs`

### Purpose
Manages OAuth2 authentication and token lifecycle.

### Structure
```rust
pub struct AuthClient {
    token_url: Url,                     // OAuth2 token endpoint
    client_id: String,                  // Application client ID
    client_secret: String,              // Application secret
    token: Arc<Mutex<Option<AuthToken>>>, // Cached token (thread-safe)
    http_client: reqwest::Client,       // HTTP client for requests
}
```

### Initialization
```rust
impl AuthClient {
    pub fn new(token_url: Url, client_id: String, client_secret: String) -> Self {
        Self {
            token_url,
            client_id,
            client_secret,
            token: Arc::new(Mutex::new(None)),
            http_client: reqwest::Client::new(),
        }
    }
}
```

### Token Request
```rust
async fn fetch_new_token(&self) -> anyhow::Result<AuthToken> {
    let params = [
        ("grant_type", "client_credentials"),
        ("client_id", &self.client_id),
        ("client_secret", &self.client_secret),
        ("scope", "all"),
    ];

    let response = self.http_client
        .post(self.token_url.as_str())
        .form(&params)
        .send()
        .await?;

    let status = response.status();

    if status == StatusCode::UNAUTHORIZED {
        error!("Authentication failed: invalid credentials");
        bail!("Authentication failed: invalid credentials");
    }

    if !status.is_success() {
        let text = response.text().await?;
        error!("Token fetch failed: status {}, body: {}", status, text);
        bail!("Token fetch failed: status {}", status);
    }

    let token_response: TokenResponse = response.json().await?;
    let token = AuthToken::new(
        token_response.access_token,
        token_response.expires_in,
    );

    Ok(token)
}
```

**TokenResponse Format**:
```json
{
  "access_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "token_type": "bearer",
  "expires_in": 3600
}
```

### Token Validation & Refresh
```rust
pub async fn get_valid_token(&self) -> anyhow::Result<String> {
    let mut token_guard = self.token.lock().await;

    // Check if cached token is still valid
    if let Some(token) = token_guard.as_ref() {
        if !token.is_expired() {
            return Ok(token.header_value());
        }
    }

    // Token expired or missing, fetch new one
    let new_token = self.fetch_new_token().await?;
    let header_value = new_token.header_value();
    *token_guard = Some(new_token);

    Ok(header_value)
}
```

**Key Features**:
- Thread-safe caching with `Arc<Mutex<>>`
- 30-second expiration buffer (refreshes before actual expiration)
- Automatic refresh on `get_valid_token()` call

### AuthToken Structure
**Location**: `src/lib/outbound/client/auth/token.rs`

```rust
pub struct AuthToken {
    access_token: String,
    expires_at: i64,  // Unix timestamp
}

impl AuthToken {
    pub fn new(access_token: String, expires_in: i64) -> Self {
        let now = Utc::now().timestamp();
        Self {
            access_token,
            expires_at: now + expires_in,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = Utc::now().timestamp();
        now >= self.expires_at - 30  // 30-second buffer
    }

    pub fn header_value(&self) -> String {
        format!("Bearer {}", self.access_token)
    }
}
```

---

## ActionClient

**Location**: `src/lib/outbound/client/action.rs`

### Purpose
Posts ActionObject instances to Halo API for import.

### Structure
```rust
pub struct ActionClient {
    base_url: Url,                  // Halo instance base URL
    auth_client: Arc<AuthClient>,   // Shared AuthClient for tokens
    http_client: reqwest::Client,   // HTTP client
}
```

### Initialization
```rust
impl ActionClient {
    pub fn new(base_url: Url, auth_client: Arc<AuthClient>) -> Self {
        Self {
            base_url,
            auth_client,
            http_client: reqwest::Client::new(),
        }
    }
}
```

### Batch Import (Primary Method)
```rust
pub async fn post_action_objects(
    &self,
    actions: Vec<ActionObject>,
) -> anyhow::Result<()> {
    // Rate limiting: 500ms delay
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Get valid token (auto-refreshes if needed)
    let mut auth_token = self.auth_client.get_valid_token().await?;

    // Build URL: /api/actions
    let url = self.base_url.join("api/actions")?;

    // Retry loop (handles 401 and 504)
    for attempt in 0..2 {
        let response = self.http_client
            .post(url.as_str())
            .header("Authorization", &auth_token)
            .header("Content-Type", "application/json")
            .json(&actions)
            .send()
            .await?;

        let status = response.status();

        // Handle 401: refresh token and retry once
        if status == StatusCode::UNAUTHORIZED && attempt == 0 {
            warn!("Received 401 Unauthorized, refreshing token and retrying");
            auth_token = self.auth_client.get_valid_token().await?;
            continue;
        }

        // Handle 504: wait 60s, refresh token, retry outer loop
        if status == StatusCode::GATEWAY_TIMEOUT {
            warn!("Received 504 Gateway Timeout, waiting 1 minute before retrying");
            tokio::time::sleep(Duration::from_secs(60)).await;
            auth_token = self.auth_client.get_valid_token().await?;
            break; // Retry outer loop
        }

        // Handle other errors
        if !status.is_success() {
            let error_text = response.text().await?;

            // Extract action IDs for error message
            let action_ids: Vec<&str> = actions.iter()
                .map(|a| a.action_id.value())
                .collect();

            error!("Action object POST failed for action ID {}: status {}, error: {}",
                   action_ids.join(", "), status, error_text);

            bail!("Action object POST failed for action ID {}: status {}, error: {}",
                  action_ids.join(", "), status, error_text);
        }

        // Success
        return Ok(());
    }

    bail!("Failed to post actions after 2 attempts");
}
```

### Request Format
**Endpoint**: `POST /api/actions`

**Headers**:
```
Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
Content-Type: application/json
```

**Body** (batch of 1):
```json
[
  {
    "__rowNum__": null,
    "_isimport": true,
    "datetime": "2024-12-24T08:23:45.000Z",
    "actionwho": "John Doe",
    "cfactionid": "4108638",
    "customfields": [
      {
        "name": "cfactionid",
        "value": "4108638"
      }
    ],
    "note": "Action note text",
    "note_html": "Action note text",
    "outcome": "Imported Note",
    "requestid": 174443,
    "result": null,
    "ticket_id": 174443,
    "who": "John Doe"
  }
]
```

**Body** (batch of 3):
```json
[
  { /* action 1 */ },
  { /* action 2 */ },
  { /* action 3 */ }
]
```

### Response Handling
**Success** (200 OK):
```json
{
  "id": 123456,
  "status": "success"
}
```
**Note**: Response format varies by Halo version. Application doesn't parse response, only checks status code.

**Failure** (400 Bad Request):
```json
{
  "error": "Ticket not found."
}
```

**Failure** (401 Unauthorized):
```json
{
  "error": "invalid_token",
  "error_description": "The access token provided is invalid"
}
```

### Rate Limiting
**Built-in Delay**: 500ms between API calls
```rust
tokio::time::sleep(Duration::from_millis(500)).await;
```

**Purpose**:
- Prevent overwhelming Halo instance
- Avoid rate limit errors
- Sustainable throughput for long-running imports

**Throughput**:
- Batch size 1: ~2 requests/second = ~120 actions/minute
- Batch size 10: ~2 requests/second = ~1200 actions/minute
- Batch size 100: ~2 requests/second = ~12000 actions/minute

---

## ReportClient

**Location**: `src/lib/inbound/client.rs`

### Purpose
Fetches existing action IDs from Halo reports for duplicate detection.

### Structure
```rust
pub struct ReportClient {
    base_url: Url,
    auth_client: Arc<AuthClient>,
    http_client: reqwest::Client,
}
```

### Initialization
```rust
impl ReportClient {
    pub fn new(base_url: Url, auth_client: &Arc<AuthClient>) -> Self {
        Self {
            base_url,
            auth_client: Arc::clone(auth_client),
            http_client: reqwest::Client::new(),
        }
    }
}
```

### Fetch Action IDs
```rust
pub async fn fetch_action_ids(&self, resource_id: &str) -> anyhow::Result<Vec<String>> {
    // Get valid token
    let auth_token = self.auth_client.get_valid_token().await?;

    // Build URL: /api/Report/{resource_id}
    let url = self.base_url.join(&format!("api/Report/{}", resource_id))?;

    // Make request
    let response = self.http_client
        .get(url.as_str())
        .header("Authorization", &auth_token)
        .send()
        .await?;

    let status = response.status();

    if !status.is_success() {
        let error_text = response.text().await?;
        error!("Report fetch failed for resource {}: status {}, error: {}",
               resource_id, status, error_text);
        bail!("Report fetch failed: status {}", status);
    }

    // Parse JSON response
    let report_data: Vec<ReportRow> = response.json().await?;

    // Extract action IDs
    let action_ids: Vec<String> = report_data
        .into_iter()
        .filter_map(|row| row.action_id)
        .map(|id| id.to_string())
        .collect();

    info!("Fetched {} action IDs from report {}", action_ids.len(), resource_id);

    Ok(action_ids)
}
```

### Report Format
**Endpoint**: `GET /api/Report/{resource_id}`

**Headers**:
```
Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...
```

**Response**:
```json
[
  {
    "action_id": "12345",
    "other_field": "value"
  },
  {
    "action_id": "67890",
    "other_field": "value"
  }
]
```

**ReportRow Structure**:
```rust
#[derive(Deserialize)]
struct ReportRow {
    action_id: Option<String>,  // Only field we care about
    #[serde(flatten)]
    _extra: HashMap<String, serde_json::Value>,  // Ignore other fields
}
```

### Usage Pattern
```rust
// Fetch IDs from multiple reports
for uuid in &config.action_ids_resource_uuids {
    let ids = report_client.fetch_action_ids(uuid).await?;
    existing_ids.extend(ids.clone());
    append_resource_to_cache(uuid, &ids)?;
}
```

**Key Features**:
- Supports multiple report UUIDs (comma-separated in config)
- Caches results to avoid re-fetching on restart
- Filters out null/missing action_id values

---

## Shared Patterns

### Error Handling
All clients use consistent error handling:
1. Check HTTP status code
2. Extract error text from response body
3. Log error with context (resource ID, status, message)
4. Bail with descriptive error

### Token Management
All clients share single AuthClient instance via `Arc<AuthClient>`:
```rust
let auth_client = Arc::new(AuthClient::new(/* ... */));
let action_client = ActionClient::new(base_url, Arc::clone(&auth_client));
let report_client = ReportClient::new(base_url, &auth_client);
```

**Benefits**:
- Single token cache shared across clients
- Automatic token refresh coordinated
- No redundant token fetches

### HTTP Client Reuse
Each client maintains its own `reqwest::Client`:
```rust
http_client: reqwest::Client::new()
```

**Benefits**:
- Connection pooling per client
- Persistent HTTP connections
- Better performance for repeated requests

---

## Configuration

### Environment Variables
```env
# Base URL (include trailing slash)
BASE_RESOURCE_URL=https://example.haloitsm.com/

# OAuth2 credentials
CLIENT_ID=your-client-id
CLIENT_SECRET=your-client-secret

# Report UUIDs (comma-separated)
ACTION_IDS_RESOURCE_PATHS=aa637f8f-0e94-48e4-8881-8e1ff08445ec,9a887d53-85fa-4928-a450-9aece690ade2

# Custom field ID for action identifier
ACTION_ID_CUSTOM_FIELD_ID=123
```

### URL Construction
```rust
// Token endpoint
token_url = base_url.join("auth/token")
// Example: https://example.haloitsm.com/auth/token

// Action import endpoint
action_url = base_url.join("api/actions")
// Example: https://example.haloitsm.com/api/actions

// Report fetch endpoint
report_url = base_url.join(&format!("api/Report/{}", resource_id))
// Example: https://example.haloitsm.com/api/Report/aa637f8f-0e94-48e4-8881-8e1ff08445ec
```

---

## Performance Characteristics

### AuthClient
- **Token lifetime**: ~3600 seconds (1 hour, configurable by Halo)
- **Refresh frequency**: Only when expired (checked before each API call)
- **Overhead**: ~100-200ms per token fetch (negligible due to caching)

### ActionClient
- **Rate limit**: 500ms delay = 2 requests/second
- **Retry overhead**:
  - 401: ~100-200ms (token refresh) + retry
  - 504: 60 seconds + retry
- **Batch performance**: Linear scaling (10x batch size = ~10x throughput)

### ReportClient
- **Fetch time**: Depends on report size
  - 100K IDs: ~5-10 seconds
  - 1M IDs: ~30-60 seconds
  - 3M IDs: ~90-120 seconds (may timeout, use multiple smaller reports)
- **Frequency**: Once per report UUID (cached thereafter)
- **Parallelization**: Could fetch multiple reports concurrently (not currently implemented)

---

## Security Considerations

### Credential Storage
- Credentials stored in `.env` file (not committed to git)
- `.env` should have restrictive permissions: `chmod 600 .env`

### Token Handling
- Tokens stored in memory only (never written to disk)
- Tokens automatically expire after configured lifetime
- No token refresh on application restart (fetches new token)

### HTTPS
- All API calls use HTTPS (enforced by Halo)
- HTTP URLs automatically upgraded to HTTPS by base_url parsing

### Error Messages
- Error logs may contain sensitive info (action IDs, ticket IDs)
- Log files should have restrictive permissions
- Avoid logging full request/response bodies (only status codes)
