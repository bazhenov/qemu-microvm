FROM debian:bookworm

RUN rm -f /sbin/init
COPY init /sbin/init

RUN apt update -y
RUN apt-get install iputils-ping net-tools isc-dhcp-client procps -y
