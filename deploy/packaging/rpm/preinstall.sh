getent group lictor >/dev/null || groupadd -r lictor
getent passwd lictor >/dev/null || useradd -r -g lictor -d /var/lib/lictor -s /sbin/nologin -c "Lictor agent" lictor
exit 0
