#include <stdio.h>

#include "ngrok.h"

void main() {
	printf("Hello, world!\n");
	
	Join* handle = start_ngrok("joshcdemo.ngrok.io", "localhost:1234");
	if(handle != NULL) {
		block(handle);
	} else {
		printf("failed to start ngrok");
	}
}
