FROM debian:bookworm

RUN apt update -y
RUN apt-get install iputils-ping net-tools isc-dhcp-client procps util-linux -y

RUN rm -f /sbin/init
COPY init.sh /sbin/init
