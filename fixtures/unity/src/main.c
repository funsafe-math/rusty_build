#include <stdio.h>

int calc_add(int a, int b);
int calc_mul(int a, int b);

int main(void) {
    printf("unity demo: %d %d\n", calc_add(20, 22), calc_mul(6, 7));
    return 0;
}
