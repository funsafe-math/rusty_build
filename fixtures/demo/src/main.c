#include <stdio.h>
int value(void);
int main(void) { printf("%d\n", value()); return value() == 12 ? 0 : 1; }
