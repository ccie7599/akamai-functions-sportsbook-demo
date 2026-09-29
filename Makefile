URL ?= $(shell cat .deploy-url 2>/dev/null)
BUILD_ENV = BUILD_TIME=$$(date -u +%Y-%m-%dT%H:%MZ) GIT_SHA=$$(git rev-parse --short HEAD 2>/dev/null || echo dev)

.PHONY: build run deploy smoke bench clean

build:                 ## Build the WebAssembly component
	$(BUILD_ENV) spin build

run: build             ## Serve locally on http://127.0.0.1:3777
	spin up --listen 127.0.0.1:3777

.env:
	@echo "QUOTE_SIGNING_KEY=$$(openssl rand -hex 32)" > .env && chmod 600 .env
	@echo "Generated .env with a new quote signing key"

deploy: .env build     ## Deploy to Akamai Functions (first run creates the app)
	@. ./.env && spin aka deploy --no-confirm --variable quote_signing_key=$$QUOTE_SIGNING_KEY \
	  $$( [ -d .spin-aka ] || echo --create-name edge-book ) | tee /dev/stderr \
	  | sed -n 's/.*\(https:\/\/[a-z0-9-]*\.fwf\.app\).*/\1/p' | head -1 > .deploy-url
	@echo "URL: $$(cat .deploy-url)"

smoke:                 ## Call every route: make smoke [URL=...]
	scripts/smoke.sh $(URL)

bench:                 ## Cold-traffic benchmark: make bench [URL=...]
	scripts/bench-cold.sh $(URL) 100

clean:
	cargo clean
