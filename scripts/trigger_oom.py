import os
import time
import multiprocessing

def allocate_memory():
    print(f"Child PID {os.getpid()} allocating memory...")
    a = []
    try:
        while True:
            a.append(' ' * 50_000_000)  # Allocate 50MB chunks
    except MemoryError:
        print(f"Child PID {os.getpid()} caught MemoryError, sleeping.")
        time.sleep(60)  # Stay alive for a while to be reaped

if __name__ == "__main__":
    print(f"Parent PID {os.getpid()} starting OOM trigger...")
    procs = [multiprocessing.Process(target=allocate_memory) for _ in range(16)]
    for p in procs:
        p.start()
    for p in procs:
        p.join()