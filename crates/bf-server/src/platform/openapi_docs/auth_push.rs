use serde_json::{Value, json};

pub(super) fn docs() -> Vec<Value> {
    let endpoint = json!({ "type": "string", "description": "The push service address from the browser's PushSubscription." });
    let credential_options_note = "Set-Cookie `__Host-bf_wa` (HttpOnly, Secure, SameSite=Strict, Path=/, 5 minutes) names the stored one-time challenge; the follow-up call must send it back.";
    vec![
        json!({
            "method": "get", "path": "/api/v1/push/key", "auth": "anyone",
            "description": "The VAPID public key for Web Push. Answers 404 when the server has no VAPID key configured (BF_VAPID_PRIVATE).",
            "response": {
                "type": "object", "required": ["key"],
                "properties": { "key": { "type": "string", "description": "The uncompressed P-256 public key, base64url without padding (the applicationServerKey)." } }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/push/subscribe", "auth": "member", "status": "204",
            "description": "Subscribe this browser to Web Push. Needs the X-CSRF-Token header. A member may have at most 10 subscriptions. 404 when Web Push is not configured; 422 when the address is not an acceptable public target (or longer than 1000 characters), the keys are invalid, or the limit is reached. No body is returned.",
            "request": {
                "type": "object", "required": ["endpoint", "keys"],
                "properties": {
                    "endpoint": { "type": "string", "maxLength": 1000, "description": "The push service address from the browser's PushSubscription. The server posts to it, so it must pass the same checks as a webhook target." },
                    "keys": {
                        "type": "object", "required": ["p256dh", "auth"],
                        "properties": {
                            "p256dh": { "type": "string", "description": "The browser's P-256 public key (uncompressed SEC1 point), base64url without padding." },
                            "auth": { "type": "string", "description": "The 16-byte authentication secret, base64url without padding." }
                        }
                    }
                }
            }
        }),
        json!({
            "method": "delete", "path": "/api/v1/push/subscribe", "auth": "member", "status": "204",
            "description": "Unsubscribe this browser from Web Push. Needs the X-CSRF-Token header. Succeeds whether or not the endpoint was subscribed. No body is returned.",
            "request": {
                "type": "object", "required": ["endpoint"],
                "properties": { "endpoint": endpoint }
            }
        }),
        json!({
            "method": "get", "path": "/api/v1/captcha/altcha", "auth": "anyone",
            "description": "A fresh self-hosted ALTCHA proof-of-work challenge, never cached (Cache-Control: no-store). The browser finds `number` (0 to maxnumber) whose SHA-256 of salt+number equals `challenge`, and sends the solution base64-encoded with the form. Each challenge works once and expires after 20 minutes. 404 unless the captcha provider setting is `altcha` and BF_CAPTCHA_SECRET is set.",
            "response": {
                "type": "object", "required": ["algorithm", "challenge", "maxnumber", "salt", "signature"],
                "properties": {
                    "algorithm": { "type": "string", "enum": ["SHA-256"] },
                    "challenge": { "type": "string", "description": "Hex SHA-256 of salt followed by the secret number." },
                    "maxnumber": { "type": "integer", "description": "Upper bound of the search (50000)." },
                    "salt": { "type": "string", "description": "A random token followed by `?expires=<unix seconds>`." },
                    "signature": { "type": "string", "description": "Hex HMAC-SHA-256 of the challenge, signed by the server." }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/passkeys/register/options", "auth": "member",
            "description": format!("WebAuthn creation options for registering a passkey. Needs a signed-in session (the viewer's session CSRF token in X-CSRF-Token) and a recent sudo confirmation. No request body. Stores a one-time challenge owned by the member. {credential_options_note}"),
            "response": {
                "type": "object",
                "required": ["challenge", "rp", "user", "pubKeyCredParams", "authenticatorSelection", "attestation", "timeout", "excludeCredentials"],
                "properties": {
                    "challenge": { "type": "string", "description": "32 random bytes, base64url." },
                    "rp": {
                        "type": "object", "required": ["id", "name"],
                        "properties": { "id": { "type": "string", "description": "The relying party id: the host of the site's base URL." }, "name": { "type": "string", "description": "The site name." } }
                    },
                    "user": {
                        "type": "object", "required": ["id", "name", "displayName"],
                        "properties": {
                            "id": { "type": "string", "description": "The member's numeric id as 8 big-endian bytes, base64url." },
                            "name": { "type": "string", "description": "The username." },
                            "displayName": { "type": "string", "description": "The username." }
                        }
                    },
                    "pubKeyCredParams": {
                        "type": "array", "description": "Always one entry: ES256.",
                        "items": { "type": "object", "required": ["type", "alg"], "properties": { "type": { "type": "string", "enum": ["public-key"] }, "alg": { "type": "integer", "enum": [-7] } } }
                    },
                    "authenticatorSelection": {
                        "type": "object",
                        "properties": { "residentKey": { "type": "string", "enum": ["preferred"] }, "userVerification": { "type": "string", "enum": ["required"] } }
                    },
                    "attestation": { "type": "string", "enum": ["none"] },
                    "timeout": { "type": "integer", "description": "Milliseconds (120000)." },
                    "excludeCredentials": {
                        "type": "array", "description": "The member's existing passkeys, so the authenticator refuses a duplicate.",
                        "items": { "type": "object", "required": ["type", "id"], "properties": { "type": { "type": "string", "enum": ["public-key"] }, "id": { "type": "string", "description": "Credential id, base64url." } } }
                    }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/passkeys/register", "auth": "member", "status": "201",
            "description": "Finish registering a passkey. Needs a signed-in session (X-CSRF-Token), a recent sudo confirmation, and the `__Host-bf_wa` challenge cookie from the options call, which is consumed once and must belong to the same member (403 otherwise). 400 when the challenge is missing or expired; 422 when the attestation fails verification or the passkey is already registered. Records an audit entry.",
            "request": {
                "type": "object", "required": ["clientDataJSON", "attestationObject"],
                "properties": {
                    "clientDataJSON": { "type": "string", "description": "The authenticator response's clientDataJSON, base64url." },
                    "attestationObject": { "type": "string", "description": "The authenticator response's attestationObject, base64url." },
                    "name": { "type": "string", "default": "", "description": "A label for the passkey; trimmed and cut to 60 characters. Empty becomes \"Passkey\"." }
                }
            },
            "response": {
                "type": "object", "required": ["ok"],
                "properties": { "ok": { "type": "boolean", "enum": [true] } }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/passkeys/login/options", "auth": "anyone",
            "description": format!("WebAuthn request options for signing in with a discoverable passkey. No session, body or CSRF token needed. No allowCredentials list is sent: the authenticator offers its own passkeys. {credential_options_note} Cache-Control: no-store."),
            "response": {
                "type": "object", "required": ["challenge", "rpId", "userVerification", "timeout"],
                "properties": {
                    "challenge": { "type": "string", "description": "32 random bytes, base64url." },
                    "rpId": { "type": "string", "description": "The relying party id: the host of the site's base URL." },
                    "userVerification": { "type": "string", "enum": ["required"] },
                    "timeout": { "type": "integer", "description": "Milliseconds (120000)." }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/passkeys/login", "auth": "anyone", "status": "303",
            "description": "Finish signing in with a passkey. Needs no session and no CSRF token, but does need the `__Host-bf_wa` challenge cookie from login/options (consumed once; 400 when missing or expired). A passkey stands in for password and second factor. On success answers 303 to `/` and sets the session cookie, and a sign-in from a new address emails the member. 404 with an application/problem+json body carrying `unknown_credential: true` and `rpId` when the credential id is not registered here (so the browser can drop the stale passkey); 422 when the assertion is not accepted; 403 when the member is suspended or the address is banned.",
            "request": {
                "type": "object", "required": ["id", "clientDataJSON", "authenticatorData", "signature"],
                "properties": {
                    "id": { "type": "string", "description": "The credential id, base64url." },
                    "clientDataJSON": { "type": "string", "description": "The assertion response's clientDataJSON, base64url." },
                    "authenticatorData": { "type": "string", "description": "The assertion response's authenticatorData, base64url." },
                    "signature": { "type": "string", "description": "The assertion signature, base64url." }
                }
            }
        }),
        json!({
            "method": "get", "path": "/api/v1/passkeys/signal", "auth": "member",
            "description": "What the browser's passkey provider should know about this account (WebAuthn Signal API): the current credential ids, so deleted passkeys can be hidden. 401/403 for a guest. Cache-Control: no-store.",
            "response": {
                "type": "object", "required": ["rpId", "userId", "allAcceptedCredentialIds", "name", "displayName"],
                "properties": {
                    "rpId": { "type": "string" },
                    "userId": { "type": "string", "description": "The member's numeric id as 8 big-endian bytes, base64url." },
                    "allAcceptedCredentialIds": { "type": "array", "items": { "type": "string", "description": "Credential id, base64url." } },
                    "name": { "type": "string", "description": "The username." },
                    "displayName": { "type": "string", "description": "The username." }
                }
            }
        }),
        json!({
            "method": "get", "path": "/api/v1/sudo", "auth": "member",
            "description": "Which proof the sudo dialog should ask for, and whether this session is already confirmed.",
            "response": {
                "type": "object", "required": ["method", "confirmed"],
                "properties": {
                    "method": { "type": "string", "enum": ["password", "code", "none"], "description": "`password`; `code` (an authenticator code, for an account with no password); or `none` (passkey or provider sign-in only: sign in again to confirm)." },
                    "confirmed": { "type": "boolean", "description": "Whether the session holds a current sudo confirmation." }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/sudo", "auth": "member", "status": "204",
            "description": "Confirm sudo for staff and sensitive actions. Needs the session CSRF token in X-CSRF-Token. On success the session is rotated into a confirmed one: the response has no body and sets the new session cookie. 422 with the reason when the password or code is wrong or after too many failed attempts (the sign-in and authenticator-code lockouts apply); 403 while impersonating.",
            "request": {
                "type": "object",
                "properties": {
                    "password": { "type": "string", "default": "", "description": "The account password, when it has one." },
                    "code": { "type": "string", "default": "", "description": "A current authenticator (TOTP) code, used only for an account with no password." }
                }
            }
        }),
        json!({
            "method": "post", "path": "/api/v1/csp-report", "auth": "anyone", "status": "204",
            "description": "Receiver for Content-Security-Policy violation reports, sent by the browser itself. Exempt from the cross-site origin check and needs no session or CSRF token. Accepts `application/csp-report` ({\"csp-report\": {...}}) or a Reporting API batch (`application/reports+json`, an array of {type, url, body}, of which the first 20 `csp-violation` entries count). Reports are counted by directive and logged in short form, never stored. Unparseable bodies are counted as unparsed and still answer 204. 413 when the body exceeds 64 KiB.",
            "request": {
                "oneOf": [
                    {
                        "type": "object", "description": "The report-uri format.",
                        "properties": {
                            "csp-report": {
                                "type": "object",
                                "properties": {
                                    "document-uri": { "type": "string" },
                                    "blocked-uri": { "type": "string" },
                                    "violated-directive": { "type": "string" },
                                    "effective-directive": { "type": "string" },
                                    "original-policy": { "type": "string" }
                                }
                            }
                        }
                    },
                    {
                        "type": "array", "description": "The Reporting API format.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "type": { "type": "string", "description": "`csp-violation` for the entries that are counted." },
                                "url": { "type": "string" },
                                "body": {
                                    "type": "object",
                                    "properties": {
                                        "effectiveDirective": { "type": "string" },
                                        "blockedURL": { "type": "string" },
                                        "documentURL": { "type": "string" }
                                    }
                                }
                            }
                        }
                    }
                ]
            }
        }),
        json!({
            "method": "get", "path": "/api/v1/openapi.json", "auth": "anyone",
            "response": { "type": "object", "description": "This OpenAPI 3.1 document." },
        }),
    ]
}
