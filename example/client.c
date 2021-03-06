#include <stdio.h>
#include "mage.h"

int main() {
	void *addr = calloc(1, 0);
	void *len = calloc(1, 0);
	char buffer[1024] = { 0 };

	printf("ffi_connect()\n");
	int sock = ffi_connect();

	printf("ffi_send(%d, hello)\n", sock);
	ffi_send(sock, "hello", 6);

	printf("ffi_recv(%d)\n", sock);
	ffi_recv(sock, buffer, 6);
	printf("%s\n", buffer);

	printf("bye");
	return 0;
}
