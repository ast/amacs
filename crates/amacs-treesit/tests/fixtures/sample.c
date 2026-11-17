#include <stdio.h>

#define MAX 10

struct point {
  int x;
  int y;
};

static int add(int a, int b)
{
  return a + b;
}

int main(int argc, char **argv) {
  int total = 0;
  for (int i = 0; i < MAX; i++) {
    if (i % 2 == 0) {
      total = add(total,
        i);
    } else {
      total--;
    }
  }
  switch (total) {
  case 0:
    printf("zero\n");
    break;
  default:
    break;
  }
  /* done */
  return 0;
}
