# OpenAI-compatible providers only in v0

Pi ships a catalog of ~20 providers with OAuth subscriptions and API keys. Tau v0 supports only OpenAI-compatible endpoints (base URL + key + model id), which covers OpenAI, OpenRouter, Groq, Ollama, llama.cpp, vLLM, and similar servers. Rationale: one API surface keeps the core minimal (a standing "lightweight" preference); the catalog, OAuth, and subscriptions can be added later without re-architecture. The exact contract (which endpoints, how reasoning maps, local-server status) is open on the map.

**Considered**: multi-provider catalog with OAuth in v0 (pi parity). Rejected: it is the heaviest part of pi, and it is not what "lightweight" means here.
