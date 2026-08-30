/**********
Copyright 1990 Regents of the University of California.  All rights reserved.
**********/

#include "ngspice/ngspice.h"
#include "asrcdefs.h"
#include "ngspice/cktdefs.h"
#include "ngspice/iferrmsg.h"
#include "ngspice/noisedef.h"
#include "ngspice/sperror.h"


int
ASRCnoise(int mode, int operation, GENmodel *genmodel, CKTcircuit *ckt,
          Ndata *data, double *OnDens)
{
    NOISEAN *job = (NOISEAN *) ckt->CKTcurJob;
    ASRCmodel *model;
    ASRCinstance *inst;

    for (model = (ASRCmodel *) genmodel; model;
         model = ASRCnextModel(model)) {
        for (inst = ASRCinstances(model); inst;
             inst = ASRCnextInstance(inst)) {
            double density, lnDensity, outputDensity;
            double *values, *derivatives;
            int i, error;

            if (!inst->ASRCnoiseGiven)
                continue;

            switch (operation) {
            case N_OPEN:
                if (job->NStpsSm != 0) {
                    if (mode == N_DENS) {
                        NOISE_ADD_OUTVAR(ckt, data, "onoise_%s%s",
                            inst->ASRCname, "_noise");
                    } else if (mode == INT_NOIZ) {
                        NOISE_ADD_OUTVAR(ckt, data,
                            "onoise_total_%s%s", inst->ASRCname, "_noise");
                        NOISE_ADD_OUTVAR(ckt, data,
                            "inoise_total_%s%s", inst->ASRCname, "_noise");
                    }
                }
                break;

            case N_CALC:
                if (mode == N_DENS) {
                    int count = inst->ASRCnoiseTree->numVars;

                    values = TMALLOC(double, count);
                    derivatives = TMALLOC(double, count);
                    for (i = 0; i < count; i++)
                        values[i] = ckt->CKTrhsOld[inst->ASRCnoiseVars[i]];

                    error = inst->ASRCnoiseTree->IFeval(inst->ASRCnoiseTree,
                        ckt->CKTgmin, &density, values, derivatives);
                    FREE(values);
                    FREE(derivatives);
                    if (error != OK)
                        return error;
                    if (!isfinite(density) || density < 0.0) {
                        SPfrontEnd->IFerrorf(ERR_FATAL,
                            "%s: noise PSD must be finite and non-negative",
                            inst->ASRCname);
                        return E_PARMVAL;
                    }

                    NevalSrc(&outputDensity, NULL, ckt, N_GAIN,
                        inst->ASRCposNode, inst->ASRCnegNode, 0.0);
                    outputDensity *= density;
                    lnDensity = log(MAX(outputDensity, N_MINLOG));
                    *OnDens += outputDensity;

                    if (data->delFreq == 0.0) {
                        inst->ASRCnVar[LNLSTDENS][0] = lnDensity;
                        if (data->freq == job->NstartFreq) {
                            inst->ASRCnVar[OUTNOIZ][0] = 0.0;
                            inst->ASRCnVar[INNOIZ][0] = 0.0;
                        }
                    } else {
                        double outputIntegral = Nintegrate(outputDensity,
                            lnDensity, inst->ASRCnVar[LNLSTDENS][0], data);
                        double inputIntegral = Nintegrate(
                            outputDensity * data->GainSqInv,
                            lnDensity + data->lnGainInv,
                            inst->ASRCnVar[LNLSTDENS][0] + data->lnGainInv,
                            data);

                        inst->ASRCnVar[LNLSTDENS][0] = lnDensity;
                        data->outNoiz += outputIntegral;
                        data->inNoise += inputIntegral;
                        if (job->NStpsSm != 0) {
                            inst->ASRCnVar[OUTNOIZ][0] += outputIntegral;
                            inst->ASRCnVar[INNOIZ][0] += inputIntegral;
                        }
                    }

                    if (data->prtSummary)
                        data->outpVector[data->outNumber++] = outputDensity;
                } else if (mode == INT_NOIZ && job->NStpsSm != 0) {
                    data->outpVector[data->outNumber++] =
                        inst->ASRCnVar[OUTNOIZ][0];
                    data->outpVector[data->outNumber++] =
                        inst->ASRCnVar[INNOIZ][0];
                }
                break;

            case N_CLOSE:
                return OK;
            }
        }
    }

    return OK;
}
