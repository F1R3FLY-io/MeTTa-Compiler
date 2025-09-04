/*********************************************/
/* An auxiliary program to suppress warnings */
/* Author: Sergey A.Kryloff                  */
/*********************************************/

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_BUFFER_SIZE (4096)
#define PATTERN_COUNT (2)

const char* PATTERN[PATTERN_COUNT]     = {"CC = gcc",    "BISON_OPTS = -t -pgrammar_"};
const char* REPLACEMENT[PATTERN_COUNT] = {"CC = gcc -w", "BISON_OPTS = -t -pgrammar_ -Wnone"};

void replace_pattern(char* line, const char* replacement, int starting_from) {
    char buffer[MAX_BUFFER_SIZE];
    strcpy(buffer, line + starting_from);
    strcpy(line, replacement);
    strcat(line, buffer);
}

int main(int argc, char* argv[]) {
    int i;
    char buffer[MAX_BUFFER_SIZE];
    int pattern_length;
    char* pattern_found;

    // printf("Enter text (Ctrl+D to end input on Unix, Ctrl+Z on Windows):\n");
    while (fgets(buffer, sizeof(buffer), stdin)) {
        for (i = 0; i < PATTERN_COUNT; i++) {
            pattern_found = strstr(buffer, PATTERN[i]);
            if (pattern_found) {
                pattern_length = (int)strlen(PATTERN[i]);
                replace_pattern(pattern_found, REPLACEMENT[i], pattern_length);
                break;
            }
        }
        printf("%s", buffer);
    }

    return EXIT_SUCCESS;
}
