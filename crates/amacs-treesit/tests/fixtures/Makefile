CC := gcc
CFLAGS += -Wall

.PHONY: all clean

all: main.o util.o
	$(CC) $(CFLAGS) -o app $^

%.o: %.c
	$(CC) -c $<

# Remove build output.
clean:
	rm -f *.o app
