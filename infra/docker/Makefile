# =============================================================================
# Autonomics Data Lake — Docker Operations
# =============================================================================
#   make up       Start all services
#   make down     Stop all services
#   make logs     Tail logs (all or one: make logs svc=garage)
#   make ps       Show service status
#   make restart  Restart one: make restart svc=garage
#   make shell    Shell into:  make shell svc=iceberg-catalog-db
#   make health   Check service health
#   make nuke     Stop + delete volumes (DESTRUCTIVE)
#
# The unified .env lives at the workspace root (../../.env) and is passed
# via --env-file so docker compose can interpolate ${VAR} values.
# =============================================================================

COMPOSE     = docker compose --env-file ../../.env
ENV_FILE    = ../../.env
SVC         ?=

GARAGE_TPL  = garage/garage.toml.template
GARAGE_OUT  = garage/garage.toml

.PHONY: check-env garage-config init up down logs ps restart shell health clean nuke
.PHONY: $(GARAGE_OUT)

check-env:
	@if [ ! -f $(ENV_FILE) ]; then \
		echo "ERROR: $(ENV_FILE) not found. Run: cp .env.example $(ENV_FILE)"; \
		exit 1; \
	fi
	@echo "env file exist, check variables integrity..."
	@./scripts/ensure-secret.sh GARAGE_RPC_SECRET  $(ENV_FILE)
	@./scripts/ensure-secret.sh GARAGE_ADMIN_TOKEN $(ENV_FILE) base64
	@./scripts/ensure-secret.sh GARAGE_METRICS_TOKEN $(ENV_FILE) base64

# Render garage.toml on the host. The dxflrs/garage image is shell-less
# (no /bin/sh), so the config must be materialised here and bind-mounted.
$(GARAGE_OUT): $(GARAGE_TPL) $(ENV_FILE) scripts/render-garage-config.sh
	@./scripts/render-garage-config.sh $(GARAGE_TPL) $(GARAGE_OUT) $(ENV_FILE)

# Alias so users can `make garage-config` to re-render on demand.
garage-config: $(GARAGE_OUT)

init: check-env $(GARAGE_OUT)
	@echo ">>> Validating compose file..."
	$(COMPOSE) config --quiet
	@echo ">>> Pulling images..."
	$(COMPOSE) pull
	@echo ">>> Setup complete. Run 'make up' to start."

up: check-env $(GARAGE_OUT)
	$(COMPOSE) up -d
	@echo ">>> Started. Lakekeeper API: http://localhost:8181"

down:
	$(COMPOSE) down

logs:
	@if [ -n "$(SVC)" ]; then \
		$(COMPOSE) logs -f $(SVC); \
	else \
		$(COMPOSE) logs -f; \
	fi

ps:
	$(COMPOSE) ps

restart:
	@if [ -z "$(SVC)" ]; then echo "Usage: make restart svc=garage"; exit 1; fi
	$(COMPOSE) restart $(SVC)

shell:
	@if [ -z "$(SVC)" ]; then echo "Usage: make shell svc=iceberg-catalog-db"; exit 1; fi
	$(COMPOSE) exec $(SVC) bash || $(COMPOSE) exec $(SVC) sh

health:
	@echo "=== Postgres ===" && docker exec iceberg-catalog-db pg_isready -U admin 2>/dev/null || echo "DOWN"
	@echo "=== Garage ==="   && curl -sf http://localhost:3903/health 2>/dev/null || echo "DOWN"
	@echo "=== Lakekeeper ===" && curl -sf http://localhost:8181/health 2>/dev/null || echo "DOWN"

clean:
	$(COMPOSE) down --remove-orphans

nuke:
	@read -p "This will DELETE ALL DATA. Type 'YES' to confirm: " confirm; \
	if [ "$${confirm}" != "YES" ]; then echo "Aborted."; exit 1; fi
	$(COMPOSE) down -v --remove-orphans
