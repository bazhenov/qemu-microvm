FROM debian:bookworm

RUN apt update -y
RUN apt-get -y install finit-sysv iputils-ping net-tools isc-dhcp-client procps util-linux curl vim

# Removing refault finit configs
RUN rm -rf /etc/finit.d
COPY finit.conf /etc/finit.conf

COPY init.sh /sbin/simple-init
