update-market-maker:
	rsync -av --progress proto-source/ market-maker/proto-source/ --exclude Cargo.toml

update-exchange-server:
	rsync -av --progress market-objects/ testing/exchange-server/market-objects/ --exclude Cargo.toml
	rsync -av --progress testing/depth-generator/ testing/exchange-server/depth-generator/ --exclude Cargo.toml
	rsync -av --progress testing/exchange-stubs/ testing/exchange-server/exchange-stubs/ --exclude Cargo.toml

build-containers:
	docker build -t exchange-server:v0.01 -f testing/exchange-server/Dockerfile .
	docker build -t market-maker:v0.01 -f market-maker/Dockerfile .
