#include <windows.h>

typedef void (*rbmod_cb)(void *ctx);

/* Runs cb(ctx) inside __try/__except. Returns 0 on success, otherwise the exception code. */
unsigned long rbmod_seh_call(rbmod_cb cb, void *ctx, unsigned long long *fault_addr)
{
    unsigned long code = 0;
    EXCEPTION_POINTERS *ep = 0;
    __try {
        cb(ctx);
    } __except (ep = GetExceptionInformation(), code = GetExceptionCode(), EXCEPTION_EXECUTE_HANDLER) {
        if (fault_addr && ep && ep->ExceptionRecord)
            *fault_addr = (unsigned long long)(ULONG_PTR)ep->ExceptionRecord->ExceptionAddress;
        return code ? code : 0xDEAD0001UL;
    }
    return 0;
}
