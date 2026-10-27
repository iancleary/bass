#define _GNU_SOURCE
#include <dlfcn.h>
#include <netdb.h>
#include <string.h>
#include <strings.h>

/* Map one documentation name to the loopback address for a bass test.
   The system resolver configuration is left unchanged. */
int getaddrinfo(const char *node, const char *service, const struct addrinfo *hints,
                struct addrinfo **result) {
    int (*real_getaddrinfo)(const char *, const char *, const struct addrinfo *,
                            struct addrinfo **) = dlsym(RTLD_NEXT, "getaddrinfo");
    if (node != NULL) {
        size_t length = strlen(node);
        if (length > 0 && node[length - 1] == '.') {
            length -= 1;
        }
        if (length == strlen("host.openshell.internal") &&
            strncasecmp(node, "host.openshell.internal", length) == 0) {
            return real_getaddrinfo("127.0.0.1", service, hints, result);
        }
    }
    return real_getaddrinfo(node, service, hints, result);
}
