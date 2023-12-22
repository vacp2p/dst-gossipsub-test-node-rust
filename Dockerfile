FROM rust:latest as build

WORKDIR /node

COPY . .

RUN cargo build --release

FROM rust:latest as build

RUN apt-get update &&

WORKDIR /node

COPY --from=build /node/target/release/node /node/node

COPY cron_runner.sh .

RUN chmod +x cron_runner.sh
RUN chmod +x node

EXPOSE 5000

ENTRYPOINT ["./cron_runner.sh"]
