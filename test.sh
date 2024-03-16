IP=192.168.124.164

cargo build
scp ./pam-any root@$IP:/etc/pam.d
scp ./target/debug/libpam_any.so root@$IP:/lib64/security