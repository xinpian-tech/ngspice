#include <math.h>

#define ICSMOM_AB_STATE 1
#define ICSMOM_AP_STATE 2
#define ICSMOM_BP_STATE 3

typedef struct {
    double capacitance;
    double dcap_dv;
} IcsMomBranch;

static IcsMomBranch
icsmom_branch(double base, double voltage, double tcoef)
{
    const double cvc1 = 1.43665e-9;
    const double cvc2 = 1.90679e-9;
    IcsMomBranch branch;

    branch.capacitance =
        0.5 * base * tcoef * (1.0 + voltage * (cvc1 + cvc2 * voltage));
    branch.dcap_dv = 0.5 * base * tcoef * (cvc1 + 2.0 * cvc2 * voltage);
    return branch;
}

static void
icsmom_initialize_state(int tag, double voltage)
{
    double *state = (double *) cm_analog_get_ptr(tag, 0);
    double *previous_state = (double *) cm_analog_get_ptr(tag, 1);

    state[0] = previous_state[0] = voltage;
    state[1] = previous_state[1] = 0.0;
}

void
cm_icsmom(ARGS)
{
    const double lf = PARAM(lf);
    const double nf = PARAM(nf);
    const double bm = PARAM(bm);
    const double tm = PARAM(tm);
    const double mr = PARAM(mr);
    const double mismod = PARAM(mismod);
    const double shielding = PARAM(shielding);
    const double dmom = PARAM(dmom);
    const double sigma_mis = PARAM(sigma_mis);
    const double nm = tm - bm + 1.0;
    const double geo_fac = 1.0 / sqrt(lf * nf * mr * nm);
    const double dc0_mis =
        1.025 * (3.45e-7 * geo_fac * geo_fac + 3.105e-6 * geo_fac +
                 2.2425e-4) * sigma_mis * mismod;
    const double lf_um = lf * 1.0e6;
    const double cf_bm1 =
        (((0.207291 * nm - 4.85047e-2) * lf_um +
          (1.05962e-2 * nm - 4.827941e-4)) * nf -
         ((9.935717e-3 * nm + 7.69893e-2) * lf_um +
          (1.65152e-3 * nm - 8.03382e-2)));
    const double cf_bmgt1 =
        (((0.199407 * nm - 3.597274e-3) * lf_um +
          (1.34357e-2 * nm - 0.018116)) * nf -
         ((1.965032e-3 * nm + 0.169599) * lf_um +
          (2.51922e-2 * nm - 0.306621)));
    const double cf_fit = cf_bm1 * (bm == 1.0) + cf_bmgt1 * (bm > 1.0);
    const double cf = fmax(cf_fit * 1.04e-15, 1.0e-18);
    const double cpara_fit =
        0.00598 * pow(bm, -0.7316) * lf_um * nf *
            ((shielding == 0.0) +
             4.1 * pow(bm, -0.85) * (shielding == 1.0)) +
        (-0.0037594 * bm + 0.0273) * lf_um * nf *
            (shielding == 2.0) + 0.2;
    const double cpara = fmax(cpara_fit * 1.0e-15, 1.0e-18);
    const double tcoef = 1.0 + 2.89e-6 * (TEMPERATURE - 25.0);
    const double main_base = mr * cf * (1.0 + dc0_mis) * (1.0 + dmom);
    const double para_base = cpara * mr;
    const double vab = INPUT(ab);
    const IcsMomBranch main_branch = icsmom_branch(main_base, vab, tcoef);
    const int has_parasitics = !PORT_NULL(ap) && !PORT_NULL(bp);

    if (PORT_NULL(ap) != PORT_NULL(bp)) {
        cm_message_send("icsmom: ap and bp must both be connected or both be null");
        OUTPUT(ab) = 0.0;
        PARTIAL(ab, ab) = 0.0;
        return;
    }

    if ((ANALYSIS != MIF_AC) && INIT) {
        cm_analog_alloc(ICSMOM_AB_STATE, 2 * sizeof(double));
        if (has_parasitics) {
            cm_analog_alloc(ICSMOM_AP_STATE, 2 * sizeof(double));
            cm_analog_alloc(ICSMOM_BP_STATE, 2 * sizeof(double));
        }
    }

    if (ANALYSIS == MIF_AC) {
        Mif_Complex_t admittance;

        admittance.real = 0.0;
        admittance.imag = RAD_FREQ * main_branch.capacitance;
        AC_GAIN(ab, ab) = admittance;

        if (has_parasitics) {
            const IcsMomBranch ap_branch =
                icsmom_branch(para_base, INPUT(ap), tcoef);
            const IcsMomBranch bp_branch =
                icsmom_branch(para_base, INPUT(bp), tcoef);

            admittance.imag = RAD_FREQ * ap_branch.capacitance;
            AC_GAIN(ap, ap) = admittance;
            admittance.imag = RAD_FREQ * bp_branch.capacitance;
            AC_GAIN(bp, bp) = admittance;
        }
    } else if (ANALYSIS == MIF_TRAN) {
        double *state;
        double dv_dt;
        double ddv_dt_dv;

        state = (double *) cm_analog_get_ptr(ICSMOM_AB_STATE, 0);
        if (cm_analog_derivative(vab, state, &dv_dt, &ddv_dt_dv) != MIF_OK)
            return;
        OUTPUT(ab) = main_branch.capacitance * dv_dt;
        PARTIAL(ab, ab) =
            main_branch.dcap_dv * dv_dt + main_branch.capacitance * ddv_dt_dv;

        if (has_parasitics) {
            const double vap = INPUT(ap);
            const double vbp = INPUT(bp);
            const IcsMomBranch ap_branch = icsmom_branch(para_base, vap, tcoef);
            const IcsMomBranch bp_branch = icsmom_branch(para_base, vbp, tcoef);

            state = (double *) cm_analog_get_ptr(ICSMOM_AP_STATE, 0);
            if (cm_analog_derivative(vap, state, &dv_dt, &ddv_dt_dv) != MIF_OK)
                return;
            OUTPUT(ap) = ap_branch.capacitance * dv_dt;
            PARTIAL(ap, ap) =
                ap_branch.dcap_dv * dv_dt + ap_branch.capacitance * ddv_dt_dv;

            state = (double *) cm_analog_get_ptr(ICSMOM_BP_STATE, 0);
            if (cm_analog_derivative(vbp, state, &dv_dt, &ddv_dt_dv) != MIF_OK)
                return;
            OUTPUT(bp) = bp_branch.capacitance * dv_dt;
            PARTIAL(bp, bp) =
                bp_branch.dcap_dv * dv_dt + bp_branch.capacitance * ddv_dt_dv;
        }
    } else if (ANALYSIS == MIF_DC) {
        icsmom_initialize_state(ICSMOM_AB_STATE, vab);
        OUTPUT(ab) = 0.0;
        PARTIAL(ab, ab) = 0.0;

        if (has_parasitics) {
            icsmom_initialize_state(ICSMOM_AP_STATE, INPUT(ap));
            icsmom_initialize_state(ICSMOM_BP_STATE, INPUT(bp));
            OUTPUT(ap) = 0.0;
            PARTIAL(ap, ap) = 0.0;
            OUTPUT(bp) = 0.0;
            PARTIAL(bp, bp) = 0.0;
        }
    } else {
        OUTPUT(ab) = 0.0;
        PARTIAL(ab, ab) = 0.0;
        if (has_parasitics) {
            OUTPUT(ap) = 0.0;
            PARTIAL(ap, ap) = 0.0;
            OUTPUT(bp) = 0.0;
            PARTIAL(bp, bp) = 0.0;
        }
    }
}
