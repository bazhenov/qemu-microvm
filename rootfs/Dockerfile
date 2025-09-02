FROM debian:bookworm

RUN apt update -y
RUN apt-get -y install finit-sysv iputils-ping net-tools isc-dhcp-client procps util-linux curl

COPY finit.conf /etc/finit.conf
RUN rm -f /var/lock

COPY init.sh /sbin/simple-init
