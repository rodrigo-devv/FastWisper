/* Driver Vulkan falso e quebrado, só para testes: o loader o carrega pelo
 * manifesto (VK_DRIVER_FILES) e a primeira chamada ao driver trava
 * (FAKE_ICD_HANG) ou aborta, como um driver instalado mas quebrado. Sem
 * core dump: o abort não deve virar alerta de crash na máquina de quem
 * roda os testes. */
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <unistd.h>

static void broken(void) {
#ifdef FAKE_ICD_HANG
    for (;;) {
        pause();
    }
#else
    prctl(PR_SET_DUMPABLE, 0, 0, 0, 0);
    abort();
#endif
}

__attribute__((visibility("default"))) int32_t
vk_icdNegotiateLoaderICDInterfaceVersion(uint32_t *version) {
    (void)version;
    broken();
    return 0;
}

__attribute__((visibility("default"))) void *
vk_icdGetInstanceProcAddr(void *instance, const char *name) {
    (void)instance;
    (void)name;
    broken();
    return NULL;
}
