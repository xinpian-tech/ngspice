#include <math.h>
#include "ngspice/const.h"

#define ICSRES_C2S_STATE 1
#define ICSRES_C1S_STATE 2

enum {
    ICSRES_TYPICAL,
    ICSRES_INVERSE,
    ICSRES_HIGH,
    ICSRES_CLAMPED_HIGH,
    ICSRES_TOP_METAL
};

enum {
    ICSRES_CAP_NONE,
    ICSRES_CAP_PHYSICAL,
    ICSRES_CAP_ASYMMETRIC,
    ICSRES_CAP_EFFECTIVE
};

typedef struct {
    double rsh;
    double rtc1;
    double rtc2;
    double dw;
    double dl;
    double ra;
    double rvc0;
    double rvc1;
    double rvc2;
    double mismatch;
    double cox;
    double capsw;
    int formula;
    int cap_geometry;
    int mismatch_multiplicative;
    int omit_mr;
    int ndif_sab_length;
} IcsResParameters;

typedef struct {
    double resistance;
    double conductance;
    double current;
    double didv;
    double weff;
    double leff;
    double c2s;
    double c1s;
} IcsResResult;

static const IcsResParameters icsres_parameters[21] = {
    {11.5,    2.08e-3,   1.77e-6,  -1.31e-8,  -2.0e-7,       .981, 1.0e-12,     3.0e-11,      1.99e-14, 0,       0,         0,         ICSRES_TYPICAL,      ICSRES_CAP_NONE,       0, 0, 0},
    {10.22,   2.543e-3, -9.401e-7, -1.6e-8,   -6.06e-8,      1,    .999e-12,    5.261e-12,    .0382e-12,0,       0,         0,         ICSRES_TYPICAL,      ICSRES_CAP_NONE,       0, 0, 0},
    {11.1,    2.19e-3,  -1.29e-7,  -9.88e-9,   0,             1,    1.0e-12,     2.0e-12,      .23e-12,  0,       1.18e-4,   7.708e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {10.252,  2.496e-3, -1.366e-6, -6.6e-9,    0,             1,    .99997e-12,  4.281e-11,    .3051e-12,0,       1.18e-4,   7.708e-11, ICSRES_TYPICAL,      ICSRES_CAP_ASYMMETRIC, 0, 0, 0},
    {962.13, -4.315e-4,  5.56e-7,   2.586e-8,  8.865e-8,      1,    8.0e-14,     2.74e-9,      4.95e-14, 8.15e-9,1.18e-4,   7.708e-11, ICSRES_INVERSE,      ICSRES_CAP_ASYMMETRIC, 1, 0, 0},
    {542.0,   1.0796e-3, 7.94e-6,   1.32e-7,  -6.65e-7,      1.0078,1.2e-13,    5.0e-9,       1.0e-15,  1.31e-6,0,         0,         ICSRES_HIGH,         ICSRES_CAP_NONE,       0, 1, 0},
    {363.0,   1.454e-3,  1.52e-6,   1.0e-7,   -3.4e-7,       .827, 1.0e-13,     4.0e-8,       1.6e-16,  6.79e-7,0,         0,         ICSRES_CLAMPED_HIGH, ICSRES_CAP_NONE,       0, 0, 0},
    {118.0,   1.27e-3,   3.03e-6,  -3.32e-10, -4.5252958e-8, .254, 5.0e-15,     2.0e-9,       1.0e-16,  6.62e-8,0,         0,         ICSRES_HIGH,         ICSRES_CAP_NONE,       0, 0, 1},
    {249.1,   1.368e-3,  7.485e-6, -1.24e-9,  -4.81e-8,      .847, 1.2e-14,     2.0e-8,       2.0e-15,  1.56e-6,0,         0,         ICSRES_HIGH,         ICSRES_CAP_NONE,       0, 0, 0},
    {272.0,  -2.57e-4,   1.46e-6,   2.0e-8,   -7.65e-9,      .771, 2.0e-13,     8.0e-9,       1.0e-16,  4.9e-6, 1.18e-4,   7.708e-11, ICSRES_INVERSE,      ICSRES_CAP_EFFECTIVE,  0, 0, 0},
    {725.7,  -3.6e-4,    1.623e-6,  1.51e-8,  -1.83e-8,      .779, 8.0e-14,     1.743e-8,     1.0e-18,  4.2e-6, 1.18e-4,   7.708e-11, ICSRES_INVERSE,      ICSRES_CAP_EFFECTIVE,  0, 0, 0},
    {.1122,   3.242e-3,  6.79e-6,   1.2485e-8, 0,             1,    1.08524e-9,  3.30805e-10,  1.48241e-12,0,     5.713e-5,  9.441e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     3.500e-5,  9.741e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     2.433e-5,  9.719e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     1.856e-5,  9.702e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     1.511e-5,  9.708e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     1.267e-5,  9.698e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     1.092e-5,  9.686e-11, ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0914,   3.126e-3,  2.349e-6,  1.246e-8,  0,             1,    1.17568e-9,  3.17021e-10,  1.60594e-12,0,     9.60e-6,   1.20e-10,  ICSRES_TYPICAL,      ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.0239,   3.635e-3, -3.27e-7,  -2.21e-9,  0,             1,    0,           0,             6.83e-5,  0,       7.667e-6,  1.310e-10, ICSRES_TOP_METAL,     ICSRES_CAP_PHYSICAL,   0, 0, 0},
    {.01929,  3.93e-3,   5.49e-6,   8.184e-8, 0,             1,    0,           0,             2.83e-5,  0,       4.519e-6,  6.993e-11, ICSRES_TOP_METAL,     ICSRES_CAP_PHYSICAL,   0, 0, 0}
};

static void
icsres_initialize_state(int tag, double voltage)
{
    double *state = (double *) cm_analog_get_ptr(tag, 0);
    double *previous_state = (double *) cm_analog_get_ptr(tag, 1);

    state[0] = previous_state[0] = voltage;
    state[1] = previous_state[1] = 0.0;
}

static int
icsres_evaluate(int variant, double l, double w, double mr,
                double resmis_mod, double flag_cc, double drsh, double ddw,
                double dcox, double dcapsw, double sigma_mis,
                double temperature, double voltage, IcsResResult *result)
{
    const IcsResParameters *p = &icsres_parameters[variant - 1];
    const double dw = p->dw + ddw;
    const double physical_length = l * .9;
    const double weff = w * p->ra * .9 - 2.0 * dw;
    const double leff = l * p->ra * .9 - 2.0 * p->dl;
    const double geo_fac = 1.0 / sqrt(weff * leff * mr);
    const double rshmis = p->mismatch * geo_fac * sigma_mis * resmis_mod;
    const double rsh = p->mismatch_multiplicative ?
        (p->rsh + drsh) * (1.0 + rshmis) : p->rsh + drsh + rshmis;
    const double delta_t = temperature - 25.0;
    const double tcoef = 1.0 + delta_t * (p->rtc1 + p->rtc2 * delta_t);
    double rbase;
    double factor;
    double dfactor_dv;

    if (!(weff > 0.0) || !(leff > 0.0) || !(rsh > 0.0) || !(tcoef > 0.0))
        return 0;

    if (p->formula == ICSRES_TOP_METAL) {
        const double rvc1 = 0.0;
        const double rvc2 = (p->rvc2 + 2.25e-10 / physical_length) /
                            physical_length;
        const double raw = 1.0 + rvc1 * fabs(voltage) +
                           rvc2 * voltage * voltage;

        rbase = rsh / mr * physical_length / weff * tcoef;
        factor = fmin(raw, 1.1);
        dfactor_dv = raw < 1.1 ?
            rvc1 * ((voltage > 0.0) - (voltage < 0.0)) +
                2.0 * rvc2 * voltage : 0.0;
    } else {
        const double rvc = fmax(0.0, (p->rvc0 + p->rvc1 * weff +
                                     p->rvc2 * l / w) / (leff * leff));
        const double x = rvc * voltage * voltage;

        rbase = rsh * (p->ndif_sab_length ?
            leff - 5.0e-28 / (l * l * l) : leff) / weff;
        if (!p->omit_mr)
            rbase /= mr;
        rbase *= tcoef;

        if (p->formula == ICSRES_TYPICAL) {
            factor = 1.5 - 1.0 / (2.0 + x);
            dfactor_dv = 2.0 * rvc * voltage / ((2.0 + x) * (2.0 + x));
        } else if (p->formula == ICSRES_INVERSE) {
            factor = .5 + 1.0 / (2.0 + x);
            dfactor_dv = -2.0 * rvc * voltage /
                         ((2.0 + x) * (2.0 + x));
        } else {
            const double raw = 2.0 - 1.0 / (1.0 + x);

            factor = raw;
            dfactor_dv = 2.0 * rvc * voltage /
                         ((1.0 + x) * (1.0 + x));
            if (p->formula == ICSRES_CLAMPED_HIGH) {
                factor = fmin(fmax(raw, .85), 1.15);
                if (raw <= .85 || raw >= 1.15)
                    dfactor_dv = 0.0;
            }
        }
    }

    result->resistance = rbase * factor;
    if (!(result->resistance > 0.0))
        return 0;
    result->conductance = 1.0 / result->resistance;
    result->current = voltage * result->conductance;
    result->didv = result->conductance - voltage * rbase * dfactor_dv /
                   (result->resistance * result->resistance);
    result->weff = weff;
    result->leff = leff;
    result->c2s = result->c1s = 0.0;

    if (p->cap_geometry != ICSRES_CAP_NONE) {
        const double cox = p->cox + dcox;
        const double capsw = (p->capsw + dcapsw) * flag_cc;
        double length2 = physical_length;
        double length1 = physical_length;

        if (p->cap_geometry == ICSRES_CAP_ASYMMETRIC)
            length1 = l;
        else if (p->cap_geometry == ICSRES_CAP_EFFECTIVE)
            length2 = length1 = leff;

        result->c2s = (cox * weff * length2 / 2.0 + capsw * weff +
                       capsw * length2) * mr;
        result->c1s = (cox * weff * length1 / 2.0 + capsw * weff +
                       capsw * length1) * mr;
    }
    return 1;
}

void
cm_icsres(ARGS)
{
    const double variant_value = PARAM(variant);
    const int variant = (int) variant_value;
    const int has_caps = !PORT_NULL(c2s) && !PORT_NULL(c1s);
    const double evaluation_voltage = ANALYSIS == NOISE ?
        STATIC_VAR(noise_voltage) : INPUT(r);
    IcsResResult result;

    if (variant_value != (double) variant || variant < 1 || variant > 21) {
        cm_message_send("icsres: variant must be an integer from 1 through 21");
        return;
    }
    if (PORT_NULL(c2s) != PORT_NULL(c1s)) {
        cm_message_send("icsres: c2s and c1s must both be connected or both be null");
        return;
    }
    if (has_caps &&
        icsres_parameters[variant - 1].cap_geometry == ICSRES_CAP_NONE) {
        cm_message_send("icsres: selected variant has no substrate-capacitance equation");
        return;
    }

    if (!icsres_evaluate(variant, PARAM(l), PARAM(w), PARAM(mr),
                         PARAM(resmis_mod), PARAM(flag_cc), PARAM(drsh),
                         PARAM(ddw), PARAM(dcox), PARAM(dcapsw),
                         PARAM(sigma_mis), TEMPERATURE, evaluation_voltage,
                         &result)) {
        cm_message_send("icsres: effective geometry, sheet resistance, and temperature factor must be positive");
        return;
    }

    if (ANALYSIS == MIF_DC || ANALYSIS == MIF_TRAN)
        STATIC_VAR(noise_voltage) = INPUT(r);

    if (ANALYSIS == NOISE) {
        const int thermal =
            cm_noise_add_source("thermal", 0, 0, MIF_NOISE_CURRENT);
        const int flicker =
            cm_noise_add_source("flicker", 0, 0, MIF_NOISE_CURRENT);

        if (!mif_private->noise->registering) {
            NOISE_DENSITY(thermal) = 4.0 * CONSTboltz *
                (TEMPERATURE + CONSTCtoK) * result.conductance;
            NOISE_DENSITY(flicker) = variant == 11 && NOISE_FREQ > 0.0 ?
                1.7e-22 * pow(fabs(result.current), 2.0) /
                    (pow(result.leff, .96) * pow(result.weff, 1.01) *
                     pow(NOISE_FREQ, 1.0)) : 0.0;
        }
        return;
    }

    if ((ANALYSIS != MIF_AC) && INIT && has_caps) {
        cm_analog_alloc(ICSRES_C2S_STATE, 2 * sizeof(double));
        cm_analog_alloc(ICSRES_C1S_STATE, 2 * sizeof(double));
    }

    if (ANALYSIS == MIF_AC) {
        Mif_Complex_t admittance;

        admittance.real = result.didv;
        admittance.imag = 0.0;
        AC_GAIN(r, r) = admittance;
        if (has_caps) {
            admittance.real = 0.0;
            admittance.imag = RAD_FREQ * result.c2s;
            AC_GAIN(c2s, c2s) = admittance;
            admittance.imag = RAD_FREQ * result.c1s;
            AC_GAIN(c1s, c1s) = admittance;
        }
    } else {
        OUTPUT(r) = result.current;
        PARTIAL(r, r) = result.didv;

        if (has_caps && ANALYSIS == MIF_TRAN) {
            double *state;
            double dv_dt;
            double ddv_dt_dv;

            state = (double *) cm_analog_get_ptr(ICSRES_C2S_STATE, 0);
            if (cm_analog_derivative(INPUT(c2s), state, &dv_dt,
                                     &ddv_dt_dv) != MIF_OK)
                return;
            OUTPUT(c2s) = result.c2s * dv_dt;
            PARTIAL(c2s, c2s) = result.c2s * ddv_dt_dv;

            state = (double *) cm_analog_get_ptr(ICSRES_C1S_STATE, 0);
            if (cm_analog_derivative(INPUT(c1s), state, &dv_dt,
                                     &ddv_dt_dv) != MIF_OK)
                return;
            OUTPUT(c1s) = result.c1s * dv_dt;
            PARTIAL(c1s, c1s) = result.c1s * ddv_dt_dv;
        } else if (has_caps) {
            OUTPUT(c2s) = 0.0;
            PARTIAL(c2s, c2s) = 0.0;
            OUTPUT(c1s) = 0.0;
            PARTIAL(c1s, c1s) = 0.0;
            if (ANALYSIS == MIF_DC) {
                icsres_initialize_state(ICSRES_C2S_STATE, INPUT(c2s));
                icsres_initialize_state(ICSRES_C1S_STATE, INPUT(c1s));
            }
        }
    }
}
