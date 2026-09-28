# API errors

Authentication, lobby, and YouTube endpoint errors share an `application/json` body:

```json
{"code":"invalid_request","error":"Readable explanation."}
```

Clients should branch on the HTTP status and stable `code`, and use `error` for display. Authentication failures use `unauthorized`; existing lobby codes are unchanged. YouTube errors now return JSON, including body validation failures (400, 413, 415, 422). Routing-level 404/405 responses are outside this contract.

`backend/src/api_error.rs` owns `ErrorResponse` (the shared OpenAPI schema) and `ApiError` (status plus body serialization). Domain error enums retain their own variants and map to `ApiError` in their HTTP response implementations. Choose static, safe messages; log internal failure details separately. Endpoint-specific cookies, cache headers, and bearer challenges remain at their existing boundaries.
