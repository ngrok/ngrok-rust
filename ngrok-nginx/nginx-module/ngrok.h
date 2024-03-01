#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct Join Join;

struct Join *start_ngrok(const char *domain, uint16_t port, const char *policy_file, const char *oauth, const char *oauth_allow_domain);

void block(struct Join *join);

void drop(struct Join *join);
