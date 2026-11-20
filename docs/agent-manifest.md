# Airo Agent Manifest Specification

## Purpose

The Agent Manifest defines the identity, capabilities, input/output schemas, privacy boundaries, and operational limits of user-owned and third-party agents operating within Airo.

---

## Agent Ownership Models

1. **User-Owned Local Agent (`user_local`):**
   - Runs on the user's host/VPS or local terminal.
   - Connects outbound to Airo via `airobot-cli`.
   - Credentials (e.g. LLM API keys) remain entirely on the user's machine.
   - Airo never sees or stores model credentials.

2. **User-Owned Cloud Agent (`user_cloud`):**
   - Configured with a user-supplied OpenAI/OpenRouter compatible endpoint.
   - Called by Airo with scoped context packs.

3. **Third-Party Provider Agent (`provider`):**
   - Hosted and managed by independent expert vendors (e.g. security auditors, legal drafters).
   - Installed into a workspace only with explicit user authorization.
   - Receives only scoped `ContextPack` data; never granted direct repo or full database access.

---

## Manifest Format (YAML / JSON)

```yaml
agent:
  name: "python-security-expert"
  version: "1.0.0"
  provider: "vendor-x"
  description: "Automated vulnerability scanner and static analysis consultant."
  owner_type: "provider" # "user_local" | "user_cloud" | "provider"
  capabilities:
    - "code_review"
    - "security_audit"
    - "dependency_check"
  input_schema:
    required:
      - "repo_summary"
      - "selected_files"
    optional:
      - "test_results"
  output_schema:
    required:
      - "findings"
      - "risk_score"
    optional:
      - "patch_suggestions"
  privacy_policy:
    no_raw_repo_access: true
    no_network_egress: true
    no_secret_storage: true
    isolated_context_only: true
  limits:
    max_tokens: 50000
    timeout_seconds: 300
  ratings:
    success_rate: 0.88
    stars: 4.5
```
