FROM rust:latest as build

WORKDIR /node

COPY . .

RUN cargo build --release

FROM rust:latest

RUN apt-get update &&  apt-get install cron -y

WORKDIR /node

COPY --from=build /node/target/release/main /node/main
COPY cron_runner.sh .

RUN chmod +x cron_runner.sh
RUN chmod +x main

EXPOSE 5000

ENTRYPOINT ["./cron_runner.sh"]
