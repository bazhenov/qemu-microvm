FROM debian:bookworm

# Installing packages
RUN apt update -y
RUN apt-get -y install finit-sysv ifupdown iputils-ping net-tools procps util-linux curl vim
RUN rm -rf /var/lib/apt

# Removing default finit configs
RUN rm -rf /etc/finit.d

# Linking finit-ctl executable in /sbin, so that is is accessible from PATH
RUN ln -s /usr/lib/finit/initctl /sbin/initctl

COPY finit.conf /etc/finit.conf
# For /etc/network/interfaces to be applied, `ifupdown` package should be installed
COPY interfaces /etc/network/interfaces
