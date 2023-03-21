#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

typedef struct Join Join;

struct Join *start_ngrok(const char *domain, const char *forward_to);

void block(struct Join *join);

void drop(struct Join *join);
