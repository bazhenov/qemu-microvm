## Building

1. Downloading, extracting Linux kernel and creating kernel config (this can be done on Linux or macOS)
   ```
   $ make linux linux/.config
   ```
2. Compile the kernel (it has to be done on Linux)
   ```
   $ make -C linux -j$(nproc) // or make -C linux -j8 for using 8 cores
   ```
