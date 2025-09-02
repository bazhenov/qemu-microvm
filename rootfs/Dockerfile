FROM debian:bookworm

RUN apt update -y
RUN apt-get -y install iputils-ping net-tools isc-dhcp-client procps util-linux curl

RUN rm -f /sbin/init
COPY init.sh /sbin/init
