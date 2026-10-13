#!/bin/sh
set -eu
case " ${RENEWED_DOMAINS:-} " in
    *" memhot.angelkang.com "*)
        /usr/sbin/nginx -t
        /bin/systemctl reload nginx
        ;;
esac
